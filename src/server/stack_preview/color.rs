//! On-demand color previews composed from persisted per-filter stack artifacts.
//!
//! The channel stacks remain the source of truth. Color jobs capture their
//! exact artifact revisions, register them to a common pixel grid, and then
//! delegate RGB/LRGB/narrowband composition to `seiza-stacking`.

use super::color_calibration::{
    self, ColorCalibrationPlan, StackColorCalibration, StackColorCalibrationBasis,
    StackColorCalibrationOutcome,
};
use super::{
    LatestStackPreviews, StackJobState, StackPreviewImageQuery, StackPreviewImageSize,
    StackPreviewManager, MAX_REMEMBERED_JOBS, SEIZA_STACKING_OUTPUT_VERSION,
};
use crate::concurrency::Stop;
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
use seiza_background::{
    BackgroundConfig, BackgroundFit, CorrectionMode, FittedModel, ModelConfig, ProtectedRegion,
};
use seiza_stacking::{
    combine_lrgb, combine_narrowband, combine_rgb, resample_to_reference, write_color_fits_f32,
    write_processed_image_fits_f32, ColorComposition, ColorCrop, ColorNormalization, ColorOptions,
    ColorTransfer, CropReport, FitsFrame, ForaxxOptions, LinearImage, NarrowbandPalette,
    ReferenceRegion, Registrar, RegistrationOptions,
};
use seiza_stretch::{ImageShape, StretchStack, StretchStackOutput, StretchStageProgress};
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

const STACK_COLOR_CACHE_VERSION: u32 = 15;
const COLOR_INPUT_CACHE_VERSION: u32 = 4;
const SEIZA_BACKGROUND_VERSION: &str = env!("PSF_GUARD_SEIZA_BACKGROUND_VERSION");
const MAX_REGISTRATION_RMS_PIXELS: f64 = 2.0;
const COLOR_BYTES_PER_PIXEL: u64 = 64;
const COLOR_DECONVOLUTION_BYTES_PER_PIXEL: u64 = 40;
/// The interleaved RGB copy color calibration measures.
const COLOR_CALIBRATION_BYTES_PER_PIXEL: u64 = 12;

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

    pub(crate) fn label(self) -> &'static str {
        self.seiza().name()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Exact current mono artifacts to use when a role has multiple stacks.
    /// Omitted roles are resolved only when there is one usable candidate.
    #[serde(default)]
    pub input_sources: BTreeMap<StackColorRole, StackColorSourceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackColorSourceRef {
    pub job_id: String,
    pub group_index: usize,
    pub artifact_revision: String,
}

/// Where a color build's inputs take their deconvolution and RC-Astro steps
/// from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorInputProcessing {
    /// The build's own `input_deconvolutions` and `input_rc_astro`. Processing
    /// saved before this choice existed reads as this, so its builds keep
    /// their results.
    #[default]
    Own,
    /// What each input's channel card has applied to the stack the build
    /// uses, read when the build is prepared. The build's own settings are
    /// replaced with it.
    Channels,
}

impl ColorInputProcessing {
    fn is_own(&self) -> bool {
        *self == Self::Own
    }
}

/// What one channel card had applied when a build that follows the cards was
/// prepared: the settings its input ran with.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FollowedChannelProcessing {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deconvolution: Option<seiza_deconvolution::DeconvolutionConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rc_astro: Option<super::rc_astro::RcAstroProcessing>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StackColorProcessing {
    /// Whether the inputs' deconvolution and RC-Astro steps are the build's
    /// own or follow each channel's card.
    #[serde(default, skip_serializing_if = "ColorInputProcessing::is_own")]
    pub input_processing: ColorInputProcessing,
    /// Optional smooth background correction applied to each linear channel
    /// independently before cross-channel registration.
    #[serde(default)]
    pub background_extraction: Option<StackBackgroundExtraction>,
    /// Optional deconvolution of selected registered linear input channels,
    /// before display normalization. Empty by default: color previews never
    /// restore pixels unless the user explicitly enables a role.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_deconvolutions: BTreeMap<StackColorRole, seiza_deconvolution::DeconvolutionConfig>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub input_rc_astro: BTreeMap<StackColorRole, super::rc_astro::RcAstroProcessing>,
    /// Ordered display stretches applied independently after registration and
    /// robust normalization of each physical input channel.
    #[serde(default)]
    pub input_stretches: BTreeMap<StackColorRole, Vec<super::stretch::StackStretchRequest>>,
    /// Ordered display stretches applied to the composed RGB result.
    #[serde(default)]
    pub output_stretches: Vec<super::stretch::StackStretchRequest>,
    /// Photometric color calibration against Gaia, on the registered linear
    /// channels before any stretch. RGB and LRGB requests without it get the
    /// defaults; narrowband palettes never calibrate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration: Option<StackColorCalibration>,
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
    RcAstroInputs,
    NormalizingInputs,
    CalibratingColor,
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
    /// How far the units in hand have got, in units, when the work can say:
    /// an RC-Astro tool's own progress. Zero between units.
    #[serde(default)]
    pub partial_units: f64,
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
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub exposure_group: Option<crate::server::exposure_groups::ExposureGroup>,
    pub job_id: String,
    pub group_index: usize,
    pub artifact_revision: String,
    pub accepted_frames: usize,
    /// WBPP made this channel's stack; it records no frame count.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub wbpp: bool,
    /// Integrated exposure of the channel stack, in seconds. `None` on a
    /// color artifact composed before the total was recorded.
    #[serde(default)]
    pub total_exposure_seconds: Option<f64>,
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
    /// Built by the automatic refresh rather than asked for.
    #[serde(default)]
    pub automatic: bool,
    pub job_id: String,
    pub database_id: String,
    pub project_id: i32,
    pub target_id: i32,
    pub target_name: String,
    pub kind: StackColorKind,
    pub palette: Option<StackNarrowbandPalette>,
    pub label: String,
    /// Stable channel-family identity, independent of artifact revisions.
    #[serde(default)]
    pub source_family_key: String,
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
    pub resolved_input_rc_astro: BTreeMap<StackColorRole, StackColorRcAstroResult>,
    #[serde(default)]
    pub input_rc_astro_ids: BTreeMap<StackColorRole, String>,
    /// For a build that follows the channel cards: what each card had
    /// applied, one entry per input, including inputs whose card had
    /// nothing. A card that has changed since makes the build out of date.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub followed_channel_processing: BTreeMap<StackColorRole, FollowedChannelProcessing>,
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
    /// The solve and Gaia catalog color calibration rests on. Part of the
    /// cache identity; absent when calibration is off.
    #[serde(default)]
    pub color_calibration_basis: Option<StackColorCalibrationBasis>,
    /// What color calibration did, once the composition has run.
    #[serde(default)]
    pub color_calibration: Option<StackColorCalibrationOutcome>,
    pub preview_url: String,
    pub fits_url: String,
    pub error: Option<String>,
    #[serde(default)]
    pub outdated: bool,
    #[serde(default)]
    pub outdated_reason: Option<String>,
    /// While its request is prepared: what was asked for, in a few words.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparing: Option<String>,
    /// The composition a request being prepared became.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub replaced_by: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackColorRcAstroResult {
    #[serde(flatten)]
    pub result: super::rc_astro::StackRcAstroResult,
    pub fits_url: String,
    pub stars_fits_url: Option<String>,
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
    pub source_candidates: Vec<StackColorSource>,
    pub ambiguous_roles: Vec<StackColorRole>,
    pub unmapped_filters: Vec<String>,
    pub rgb_available: bool,
    pub lrgb_available: bool,
    pub narrowband_palettes: Vec<StackNarrowbandPalette>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct LatestStackColorPreviews {
    schema_version: u32,
    database_id: String,
    project_id: i32,
    updated_unix_seconds: i64,
    pub(crate) jobs: Vec<StackColorJob>,
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
    #[serde(default)]
    resolved_rc_astro: BTreeMap<StackColorRole, StackColorRcAstroResult>,
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

#[derive(Serialize, Deserialize)]
pub(super) struct PreparedColorJob {
    pub(super) public: StackColorJob,
    pub(super) stack_root: PathBuf,
    background_regions: BTreeMap<StackColorRole, Vec<ProtectedRegion>>,
    rc_astro_schemas: BTreeMap<StackColorRole, Vec<(String, seiza_stacking::ExternalToolSchema)>>,
    rc_astro_chains: BTreeMap<StackColorRole, Vec<String>>,
    color_calibration: Option<ColorCalibrationPlan>,
}

impl PreparedColorJob {
    /// Whether its prepared inputs (the registered, background-corrected
    /// channels, with any deconvolution and RC-Astro results) are cached
    /// whole, so composing reuses them.
    fn inputs_cached(&self) -> bool {
        self.public
            .linear_input_id
            .as_deref()
            .is_some_and(|input_id| {
                cached_color_inputs(
                    &self.stack_root,
                    input_id,
                    &self.public.sources,
                    &self.public.input_rc_astro_ids,
                )
                .is_some()
            })
    }

    /// Whether the composition prepares nothing at length: its inputs are
    /// cached, or it asks for neither deconvolution nor RC-Astro.
    pub(super) fn runs_quickly(&self) -> bool {
        let deconvolves = self
            .public
            .processing
            .as_ref()
            .is_some_and(|processing| !processing.input_deconvolutions.is_empty());
        self.inputs_cached() || (!deconvolves && self.rc_astro_chains.is_empty())
    }
}

/// Whether a composition runs in the quick lane, beside the stack line: a
/// person's, preparing nothing at length. One whose inputs another queued or
/// running composition is preparing now waits in the line instead, so two
/// never write the same cached inputs at once. Automatic compositions follow
/// the builds they depend on in the line.
fn takes_quick_lane(state: &AppState, prepared: &PreparedColorJob) -> bool {
    if prepared.public.automatic || !prepared.runs_quickly() {
        return false;
    }
    prepared.inputs_cached()
        || !prepared
            .public
            .linear_input_id
            .as_deref()
            .is_some_and(|input_id| {
                state
                    .stack_previews
                    .color_inputs_in_flight(input_id, &prepared.public.job_id)
            })
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
    // One unit per RC-Astro step: a tool runs for minutes where most units
    // take seconds, so a channel's chain counts as its length.
    let rc_astro_units = processing
        .map(|processing| {
            processing
                .input_rc_astro
                .values()
                .map(|config| config.steps.len())
                .sum::<usize>()
        })
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
            StackColorProgressPhase::RcAstroInputs,
            "Processing input channels with RC-Astro",
            rc_astro_units,
        ),
        (
            StackColorProgressPhase::NormalizingInputs,
            "Normalizing input channels",
            channel_count,
        ),
        (
            StackColorProgressPhase::CalibratingColor,
            "Color calibration",
            1,
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
    shape: ImageShape,
    mut progress: impl FnMut(StretchStageProgress),
) -> Result<StretchStackOutput<f32>, seiza_stretch::StretchError> {
    let stage_count = stack.len();
    let mut current = data.to_vec();
    let mut plans = Vec::with_capacity(stage_count);
    for (stage_index, config) in stack.stages().iter().cloned().enumerate() {
        let output = StretchStack::single(config).apply_f32_image_with_progress(
            &current,
            shape,
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
            job.progress.partial_units = 0.0;
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
            job.progress.partial_units = 0.0;
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
            job.progress.partial_units = 0.0;
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
            job.progress.partial_units = 0.0;
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

    /// The share of the next `units` units already done, for work that
    /// reports its own progress.
    fn partial(&self, units: usize, fraction: f32) {
        let done = f64::from(fraction.clamp(0.0, 1.0)) * units as f64;
        self.state.stack_previews.update_color(self.job_id, |job| {
            job.progress.partial_units = done;
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
    /// Whether another queued or running composition than `except` prepares
    /// the inputs `input_id` names.
    fn color_inputs_in_flight(&self, input_id: &str, except: &str) -> bool {
        self.color_jobs.lock().unwrap().values().any(|job| {
            job.job_id != except
                && matches!(job.state, StackJobState::Queued | StackJobState::Running)
                && job.linear_input_id.as_deref() == Some(input_id)
        })
    }

    pub(super) fn get_color(&self, job_id: &str) -> Option<StackColorJob> {
        self.color_jobs.lock().unwrap().get(job_id).cloned()
    }

    pub(super) fn insert_color(&self, job: StackColorJob) -> bool {
        let mut jobs = self.color_jobs.lock().unwrap();
        if jobs.len() >= MAX_REMEMBERED_JOBS && !jobs.contains_key(&job.job_id) {
            let Some(oldest) = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.state,
                        StackJobState::Completed
                            | StackJobState::Failed
                            | StackJobState::Cancelled
                            | StackJobState::Replaced
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

    pub(super) fn update_color(&self, job_id: &str, update: impl FnOnce(&mut StackColorJob)) {
        if let Some(job) = self.color_jobs.lock().unwrap().get_mut(job_id) {
            update(job);
        }
    }

    fn persist_latest_color(&self, stack_root: &FsPath, job: &StackColorJob) -> Result<(), String> {
        let _guard = self.latest_write.lock().unwrap();
        persist_latest_color(stack_root, job)
    }
}

pub(super) fn load_persisted_color_job(
    stack_root: &FsPath,
    job_id: &str,
) -> Result<StackColorJob, AppError> {
    let bytes =
        std::fs::read(color_manifest_path(stack_root, job_id)).map_err(|_| AppError::NotFound)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid color manifest: {error}")))
}

pub async fn get_stack_color_catalog(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<StackColorCatalog>>, AppError> {
    let latest = load_latest_stacks(&ctx, project_id)?;
    let sources = collect_sources(&ctx.stack_root, &latest);
    let targets = availability(&sources);
    let mut jobs = load_latest_colors(&ctx, project_id)?.jobs;
    for job in &mut jobs {
        job.outdated_reason = color_job_outdated_reason(&ctx, &state.astrometry, job, &latest)?;
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

/// `POST /api/db/{db}/projects/{project}/stack-previews/color` — a color
/// composition. It joins the line at once, as a composition being prepared:
/// `202` and that composition, which a client follows through `replaced_by`
/// once it is prepared. While the target's channels are still building it
/// waits for them, then composes from their new stacks.
pub async fn start_stack_color(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<StackColorRequest>,
) -> Result<(StatusCode, Json<ApiResponse<StackColorJob>>), AppError> {
    validate_request(&request)?;
    let job = super::build_set::queue_color_preparation(&state, &ctx.0, project_id, request)?;
    Ok((StatusCode::ACCEPTED, Json(ApiResponse::success(job))))
}

/// The composition a request is while it is prepared, under `job_id`.
pub(super) fn preparing_color_job(
    database_id: &str,
    project_id: i32,
    request: &StackColorRequest,
    job_id: String,
) -> StackColorJob {
    let label = composition_label(request.kind, request.palette).to_string();
    let channels = required_roles(request.kind, request.palette).len();
    StackColorJob {
        automatic: false,
        schema_version: 1,
        job_id,
        database_id: database_id.to_string(),
        project_id,
        target_id: request.target_id,
        target_name: String::new(),
        kind: request.kind,
        palette: request.palette,
        preparing: Some(label.clone()),
        label,
        source_family_key: String::new(),
        state: StackJobState::Preparing,
        phase: "Preparing".into(),
        processed_channels: 0,
        total_channels: channels,
        progress: StackColorProgress::default(),
        created_unix_seconds: chrono::Utc::now().timestamp(),
        artifact_revision: String::new(),
        cache_version: STACK_COLOR_CACHE_VERSION,
        stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
        background_version: SEIZA_BACKGROUND_VERSION.into(),
        deconvolution_version: String::new(),
        linear_input_id: None,
        sources: Vec::new(),
        crop: request.crop,
        crop_report: None,
        processing: request.processing.clone(),
        resolved_input_stretches: BTreeMap::new(),
        resolved_input_deconvolutions: BTreeMap::new(),
        resolved_input_rc_astro: BTreeMap::new(),
        input_rc_astro_ids: BTreeMap::new(),
        followed_channel_processing: BTreeMap::new(),
        resolved_output_stretches: Vec::new(),
        resolved_backgrounds: BTreeMap::new(),
        resolved_background_protection: BTreeMap::new(),
        background_protection_fallbacks: BTreeMap::new(),
        color_calibration_basis: None,
        color_calibration: None,
        preview_url: String::new(),
        fits_url: String::new(),
        error: None,
        outdated: false,
        outdated_reason: None,
        replaced_by: Vec::new(),
    }
}

/// Queue a prepared composition, or hand back the same one already queued,
/// running, or finished. A person's composition that takes the worker makes
/// automatic work yield; one in the quick lane runs beside the builds.
pub(super) fn start_prepared_color(
    state: &Arc<AppState>,
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    project_id: i32,
    request: &StackColorRequest,
    mut prepared: PreparedColorJob,
    placement: super::build_set::Placement,
) -> Result<StackColorJob, AppError> {
    let person = !placement.automatic;
    if let Some(existing) = state.stack_previews.get_color(&prepared.public.job_id) {
        if matches!(
            existing.state,
            StackJobState::Queued | StackJobState::Running
        ) {
            // The same composition is already under way for the automatic
            // refresh: it is now the user's, and other automatic work yields.
            if existing.automatic && person {
                state.stack_previews.adopt(&existing.job_id);
                super::interrupt_automatic(state);
            }
            return Ok(state
                .stack_previews
                .get_color(&existing.job_id)
                .unwrap_or(existing));
        }
        if !request.force
            && existing.state == StackJobState::Completed
            && color_job_artifacts_exist(&prepared.stack_root, &existing)
        {
            let existing = mark_color_reused(existing);
            state
                .stack_previews
                .persist_latest_color(&prepared.stack_root, &existing)
                .map_err(AppError::InternalError)?;
            let _ = state.stack_previews.insert_color(existing.clone());
            return Ok(existing);
        }
    }
    let manifest = color_manifest_path(&prepared.stack_root, &prepared.public.job_id);
    if !request.force
        && let Ok(bytes) = std::fs::read(&manifest)
        && let Ok(existing) = serde_json::from_slice::<StackColorJob>(&bytes)
        && existing.state == StackJobState::Completed
        && color_job_artifacts_exist(&prepared.stack_root, &existing)
    {
        let existing = mark_color_reused(existing);
        state
            .stack_previews
            .persist_latest_color(&prepared.stack_root, &existing)
            .map_err(AppError::InternalError)?;
        let _ = state.stack_previews.insert_color(existing.clone());
        return Ok(existing);
    }

    prepared.public.automatic = placement.automatic;
    let response = prepared.public.clone();
    if !state.stack_previews.insert_color(response.clone()) {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_REMEMBERED_JOBS} color preview jobs may be active at once"
        )));
    }
    let origin = super::journal::JournaledStackJob::color(
        &ctx.id,
        project_id,
        request,
        Some(&response.job_id),
        placement.automatic,
    );
    // Real work is queued. A composition in the quick lane runs beside the
    // builds; one in the line takes the worker, so automatic work yields.
    if !enqueue_color_job(Arc::clone(state), prepared, request, origin, placement.slot) && person {
        super::interrupt_automatic(state);
    }
    Ok(response)
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
    let bytes = std::fs::read(color_manifest_path(&ctx.stack_root, &job_id))
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
        StackPreviewImageSize::Screen => color_preview_path(&ctx.stack_root, &job_id),
        StackPreviewImageSize::Original => color_original_preview_path(&ctx.stack_root, &job_id),
    };
    stream_artifact(path, "image/png", None).await
}

pub async fn download_stack_color_fits(
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    super::validate_job_id(&job_id)?;
    let manifest = std::fs::read(color_manifest_path(&ctx.stack_root, &job_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StackColorJob>(&bytes).ok());
    let label = manifest
        .as_ref()
        .map(|job| job.label.to_ascii_lowercase().replace('-', "_"))
        .unwrap_or_else(|| "color".into());
    let filename = format!("psf-guard-{label}-{}.fits", &job_id[..12]);
    stream_artifact(
        color_fits_path(&ctx.stack_root, &job_id),
        "application/fits",
        Some(filename),
    )
    .await
}

pub(super) async fn stream_artifact(
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
            let required = required_roles(request.kind, request.palette);
            if let Some(role) = request
                .input_sources
                .keys()
                .find(|role| !required.contains(role))
            {
                return Err(AppError::BadRequest(format!(
                    "{} is not an input to {}",
                    role.label(),
                    composition_label(request.kind, request.palette)
                )));
            }
            let Some(processing) = &request.processing else {
                return Ok(());
            };
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
            if let Some(calibration) = &processing.color_calibration {
                calibration.validate().map_err(AppError::BadRequest)?;
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
            for (role, config) in &processing.input_rc_astro {
                if !required.contains(role) {
                    return Err(AppError::BadRequest(format!(
                        "{} is not an input to {}",
                        role.label(),
                        composition_label(request.kind, request.palette)
                    )));
                }
                config.validate().map_err(AppError::BadRequest)?;
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

pub(super) fn prepare_color_job(
    ctx: &crate::server::database_context::DatabaseContext,
    astrometry: &crate::astrometry::AstrometryContext,
    project_id: i32,
    request: &StackColorRequest,
) -> Result<PreparedColorJob, AppError> {
    let mut request = request.clone();
    let latest = load_latest_stacks(ctx, project_id)?;
    let targets = collect_sources(&ctx.stack_root, &latest);
    let target = targets.get(&request.target_id).ok_or_else(|| {
        AppError::BadRequest("No completed channel stacks are available for that target".into())
    })?;
    let sources = select_sources(target, &request)?;
    // Following the cards needs the stacks chosen first: each input takes
    // what its card applied to that exact stack revision.
    let mut followed_channel_processing = BTreeMap::new();
    if let Some(processing) = &mut request.processing
        && processing.input_processing == ColorInputProcessing::Channels
    {
        followed_channel_processing = channel_card_processing(&ctx.stack_root, &sources)?;
        follow_channel_cards(processing, &followed_channel_processing);
    }
    let mut rc_astro_schemas = BTreeMap::new();
    if let Some(processing) = &mut request.processing {
        resolve_color_calibration(request.kind, processing);
        for (role, config) in &mut processing.input_rc_astro {
            let schemas = super::rc_astro::schemas_for(config).map_err(AppError::BadRequest)?;
            *config = super::rc_astro::normalize_against_schemas(config, &schemas)
                .map_err(AppError::BadRequest)?;
            rc_astro_schemas.insert(*role, schemas);
        }
    }
    let source_family_key = source_family_key(&sources);
    let color_calibration = request
        .processing
        .as_ref()
        .and_then(|processing| processing.color_calibration)
        .filter(|settings| settings.enabled)
        .map(|settings| plan_color_calibration(ctx, astrometry, request.kind, &sources, settings));
    let color_calibration_basis = color_calibration.as_ref().map(|plan| plan.basis.clone());

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
    let mut linear_input_id = request
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
    // Each channel's chain keys on that channel's own inputs, never on
    // RC-Astro or display stretches: changing a later tool or a stretch
    // reuses the matching chain prefix, and another channel's new frames
    // reuse it whole.
    let mut rc_astro_chains = BTreeMap::new();
    if let (Some(processing), Some(upstream_id)) = (&request.processing, &linear_input_id) {
        rc_astro_chains = color_rc_astro_chains(
            &ctx.id,
            project_id,
            request.target_id,
            request.kind,
            processing,
            &sources,
            &background_protection_summary,
            &rc_astro_schemas,
        )?;
        if !rc_astro_chains.is_empty() {
            linear_input_id = Some(rc_astro_input_cache_id(upstream_id, &rc_astro_chains)?);
        }
    }
    let job_id = color_job_id(&ColorJobIdentity {
        database_id: &ctx.id,
        project_id,
        request: &request,
        label: &label,
        rc_astro_input_id: (!rc_astro_chains.is_empty()).then(|| {
            linear_input_id
                .as_deref()
                .expect("RC-Astro requires processed inputs")
        }),
        background_protection: &background_protection_summary,
        color_calibration_basis: color_calibration_basis.as_ref(),
        sources: &sources,
    })?;
    let artifact_revision = super::new_artifact_revision();
    let total_channels = sources.len();
    Ok(PreparedColorJob {
        public: StackColorJob {
            automatic: false,
            schema_version: 1,
            job_id: job_id.clone(),
            database_id: ctx.id.clone(),
            project_id,
            target_id: request.target_id,
            target_name: target.target_name.clone(),
            kind: request.kind,
            palette: request.palette,
            label,
            source_family_key,
            state: StackJobState::Queued,
            phase: "Waiting for color processor".into(),
            processed_channels: 0,
            total_channels,
            progress: color_progress(total_channels, request.processing.as_ref()),
            created_unix_seconds: chrono::Utc::now().timestamp(),
            artifact_revision: artifact_revision.clone(),
            cache_version: STACK_COLOR_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
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
            resolved_input_rc_astro: BTreeMap::new(),
            input_rc_astro_ids: rc_astro_chains
                .iter()
                .filter_map(|(role, chain)| chain.last().map(|id| (*role, id.clone())))
                .collect(),
            followed_channel_processing,
            resolved_output_stretches: Vec::new(),
            resolved_backgrounds: BTreeMap::new(),
            resolved_background_protection: background_protection_summary,
            background_protection_fallbacks: BTreeMap::new(),
            color_calibration_basis,
            color_calibration: None,
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
            preparing: None,
            replaced_by: Vec::new(),
        },
        stack_root: ctx.stack_root.clone(),
        background_regions,
        rc_astro_schemas,
        rc_astro_chains,
        color_calibration,
    })
}

/// Plan calibration onto the reference channel's grid, which every other
/// channel is registered to: its stack's reference frame, that frame's fresh
/// pixel solve, and the stack's sky layout.
fn plan_color_calibration(
    ctx: &crate::server::database_context::DatabaseContext,
    astrometry: &crate::astrometry::AstrometryContext,
    kind: StackColorKind,
    sources: &[StackColorSource],
    settings: StackColorCalibration,
) -> ColorCalibrationPlan {
    let reference = sources
        .iter()
        .find(|source| source.role == reference_role(kind));
    let reference_image_id = reference.and_then(|source| source.reference_image_id);
    let solution = reference_image_id
        .and_then(|image_id| {
            ctx.astrometry_evidence
                .evidence_for_source(&ctx.cache_dir_path, image_id, None)
        })
        .and_then(|analysis| analysis.solution);
    color_calibration::plan(
        settings,
        reference_image_id,
        reference.and_then(|source| source.sky_orientation.as_ref()),
        solution.as_ref(),
        astrometry.gaia_photometry_signature(),
    )
}

/// Everything a color job's pixels depend on: its cache identity.
struct ColorJobIdentity<'a> {
    database_id: &'a str,
    project_id: i32,
    request: &'a StackColorRequest,
    label: &'a str,
    /// The prepared inputs' identity, when RC-Astro chains feed them.
    rc_astro_input_id: Option<&'a str>,
    background_protection: &'a BTreeMap<StackColorRole, StackBackgroundProtection>,
    color_calibration_basis: Option<&'a StackColorCalibrationBasis>,
    sources: &'a [StackColorSource],
}

fn color_job_id(identity: &ColorJobIdentity<'_>) -> Result<String, AppError> {
    let request = identity.request;
    let mut hasher = Sha256::new();
    hasher.update(identity.database_id.as_bytes());
    hasher.update(identity.project_id.to_le_bytes());
    hasher.update(request.target_id.to_le_bytes());
    hasher.update(identity.label.as_bytes());
    hasher.update(STACK_COLOR_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_OUTPUT_VERSION.as_bytes());
    hasher.update(SEIZA_BACKGROUND_VERSION.as_bytes());
    if let Some(input_id) = identity.rc_astro_input_id {
        hasher.update(input_id.as_bytes());
    }
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
    // A build that follows the cards is a job of its own even when its tools
    // match a build's own settings: a finished job is reused as it stands,
    // and it records whether it follows the cards. The color inputs and
    // RC-Astro chains key on the tools alone, so switching only composes
    // again.
    hasher.update(serde_json::to_vec(&request.processing).map_err(|error| {
        AppError::InternalError(format!(
            "Failed to encode color processing options: {error}"
        ))
    })?);
    hasher.update(
        serde_json::to_vec(identity.background_protection).map_err(|error| {
            AppError::InternalError(format!("Failed to encode background protection: {error}"))
        })?,
    );
    hasher.update(
        serde_json::to_vec(&identity.color_calibration_basis).map_err(|error| {
            AppError::InternalError(format!("Failed to encode color calibration basis: {error}"))
        })?,
    );
    for source in identity.sources {
        hasher.update([source.role as u8]);
        hasher.update(source.filter_name.as_bytes());
        hasher.update(source.job_id.as_bytes());
        hasher.update(source.group_index.to_le_bytes());
        hasher.update(source.artifact_revision.as_bytes());
    }
    let mut job_id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut job_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(job_id)
}

/// What each input's channel card has applied to the stack revision a build
/// uses: its deconvolution and RC-Astro steps. An input whose card shows no
/// processing, or only a stretch, gets an empty entry, so a card that later
/// adds a tool makes the build out of date.
pub(super) fn channel_card_processing(
    stack_root: &FsPath,
    sources: &[StackColorSource],
) -> Result<BTreeMap<StackColorRole, FollowedChannelProcessing>, AppError> {
    sources
        .iter()
        .map(|source| {
            let applied = super::stretch::applied_request(
                stack_root,
                &super::stretch::mono_source_key(&source.job_id, source.group_index),
                &source.artifact_revision,
            )?;
            let followed = applied
                .map(|request| FollowedChannelProcessing {
                    deconvolution: request.deconvolution,
                    rc_astro: request.rc_astro.filter(|config| !config.steps.is_empty()),
                })
                .unwrap_or_default();
            Ok((source.role, followed))
        })
        .collect()
}

/// Replace the inputs' deconvolution and RC-Astro steps with what their
/// channel cards applied. An input whose card applied neither gets none.
pub(super) fn follow_channel_cards(
    processing: &mut StackColorProcessing,
    followed: &BTreeMap<StackColorRole, FollowedChannelProcessing>,
) {
    processing.input_deconvolutions = followed
        .iter()
        .filter_map(|(role, card)| card.deconvolution.map(|config| (*role, config)))
        .collect();
    processing.input_rc_astro = followed
        .iter()
        .filter_map(|(role, card)| card.rc_astro.clone().map(|config| (*role, config)))
        .collect();
}

/// Spell a request's color calibration out, so the job and its cache key
/// say what runs: on by default for RGB and LRGB, never for a false-color
/// palette.
fn resolve_color_calibration(kind: StackColorKind, processing: &mut StackColorProcessing) {
    processing.color_calibration = match kind {
        StackColorKind::Rgb | StackColorKind::Lrgb => {
            Some(processing.color_calibration.unwrap_or_default())
        }
        StackColorKind::Narrowband => None,
    };
}

fn select_sources(
    target: &TargetSources,
    request: &StackColorRequest,
) -> Result<Vec<StackColorSource>, AppError> {
    let roles = required_roles(request.kind, request.palette);
    let mut sources = Vec::with_capacity(roles.len());
    for role in roles {
        let candidates = target.by_role.get(&role).map(Vec::as_slice).unwrap_or(&[]);
        if let Some(selected) = request.input_sources.get(&role) {
            let source = candidates.iter().find(|source| {
                source.job_id == selected.job_id
                    && source.group_index == selected.group_index
                    && source.artifact_revision == selected.artifact_revision
            });
            let Some(source) = source else {
                return Err(AppError::BadRequest(format!(
                    "The selected {} stack is not a current source for this target; refresh the channel selection",
                    role.label()
                )));
            };
            sources.push(source.clone());
            continue;
        }
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
                "{} has multiple {} channel stacks; select the source exposure group explicitly",
                target.target_name,
                role.label()
            )))
            }
        }
    }
    Ok(sources)
}

fn source_family_key(sources: &[StackColorSource]) -> String {
    let wbpp = sources.iter().any(|source| source.wbpp);
    // Preserve the pre-grouping latest identity for legacy unsplit projects.
    if !wbpp && sources.iter().all(|source| source.exposure_group.is_none()) {
        return String::new();
    }
    let families = sources
        .iter()
        .map(|source| {
            (
                source.role,
                (
                    source.filter_name.as_str(),
                    source
                        .exposure_group
                        .as_ref()
                        .map(|group| group.key.as_str()),
                ),
            )
        })
        .collect::<BTreeMap<_, _>>();
    // A WBPP master and PSF Guard's stack of the same filter are different
    // families, so neither composite replaces the other. Keys without WBPP
    // channels stay as they were.
    let bytes = if wbpp {
        let origins = sources
            .iter()
            .map(|source| (source.role, source.wbpp))
            .collect::<BTreeMap<_, _>>();
        serde_json::to_vec(&("wbpp-origin", &families, &origins))
    } else {
        serde_json::to_vec(&families)
    }
    .expect("color source families are serializable");
    let mut key = String::with_capacity(64);
    for byte in Sha256::digest(bytes) {
        write!(&mut key, "{byte:02x}").expect("writing to a String cannot fail");
    }
    key
}

/// The channel every other channel registers onto.
fn reference_role(kind: StackColorKind) -> StackColorRole {
    match kind {
        StackColorKind::Rgb => StackColorRole::Red,
        StackColorKind::Lrgb => StackColorRole::Luminance,
        StackColorKind::Narrowband => StackColorRole::Ha,
    }
}

/// What one channel's registered, deconvolved pixels depend on, and nothing
/// else: its own stack, the reference stack it registers onto, and its own
/// deconvolution. Background extraction picks one model for every channel,
/// so with it on, every channel's stack and protection count too.
///
/// RC-Astro chains cache under this, so a new frame in one channel reruns
/// the tools on that channel alone.
#[allow(clippy::too_many_arguments)]
fn color_role_input_id(
    database_id: &str,
    project_id: i32,
    target_id: i32,
    role: StackColorRole,
    reference_role: StackColorRole,
    processing: &StackColorProcessing,
    sources: &[StackColorSource],
    resolved_background_protection: &BTreeMap<StackColorRole, StackBackgroundProtection>,
) -> Result<String, AppError> {
    fn encode(value: &impl Serialize) -> Result<Vec<u8>, AppError> {
        serde_json::to_vec(value).map_err(|error| {
            AppError::InternalError(format!("Failed to encode color input processing: {error}"))
        })
    }
    let mut hasher = Sha256::new();
    hasher.update(b"color-role-input");
    hasher.update(database_id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update(target_id.to_le_bytes());
    hasher.update(COLOR_INPUT_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_OUTPUT_VERSION.as_bytes());
    hasher.update([role as u8, reference_role as u8]);
    let hash_source = |hasher: &mut Sha256, source: &StackColorSource| {
        hasher.update([source.role as u8]);
        hasher.update(source.filter_name.as_bytes());
        hasher.update(source.job_id.as_bytes());
        hasher.update(source.group_index.to_le_bytes());
        hasher.update(source.artifact_revision.as_bytes());
    };
    if let Some(extraction) = &processing.background_extraction {
        hasher.update(SEIZA_BACKGROUND_VERSION.as_bytes());
        hasher.update(encode(&(extraction, resolved_background_protection))?);
        for source in sources {
            hash_source(&mut hasher, source);
        }
    } else {
        for source in sources
            .iter()
            .filter(|source| source.role == role || source.role == reference_role)
        {
            hash_source(&mut hasher, source);
        }
    }
    if let Some(deconvolution) = processing.input_deconvolutions.get(&role) {
        hasher.update(seiza_deconvolution::ALGORITHM_VERSION.to_le_bytes());
        hasher.update(encode(deconvolution)?);
    }
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(id)
}

#[allow(clippy::too_many_arguments)]
fn color_rc_astro_chains(
    database_id: &str,
    project_id: i32,
    target_id: i32,
    kind: StackColorKind,
    processing: &StackColorProcessing,
    sources: &[StackColorSource],
    resolved_background_protection: &BTreeMap<StackColorRole, StackBackgroundProtection>,
    schemas: &BTreeMap<StackColorRole, Vec<(String, seiza_stacking::ExternalToolSchema)>>,
) -> Result<BTreeMap<StackColorRole, Vec<String>>, AppError> {
    processing
        .input_rc_astro
        .iter()
        .map(|(role, config)| {
            let schemas = schemas.get(role).ok_or_else(|| {
                AppError::InternalError("RC-Astro input schemas are missing".into())
            })?;
            let input_id = color_role_input_id(
                database_id,
                project_id,
                target_id,
                *role,
                reference_role(kind),
                processing,
                sources,
                resolved_background_protection,
            )?;
            let chain = super::rc_astro::rc_astro_chain_ids(
                database_id,
                &format!("color-input:{input_id}:{}", role.label()),
                &input_id,
                config,
                schemas,
                None,
            )?;
            Ok((*role, chain))
        })
        .collect()
}

fn rc_astro_input_cache_id(
    upstream_id: &str,
    chains: &BTreeMap<StackColorRole, Vec<String>>,
) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(upstream_id.as_bytes());
    hasher.update(
        serde_json::to_vec(chains).map_err(|error| AppError::InternalError(error.to_string()))?,
    );
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(id)
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
    hasher.update(SEIZA_STACKING_OUTPUT_VERSION.as_bytes());
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

/// Queue a composition: in the quick lane when [`takes_quick_lane`] says so,
/// otherwise in the stack line. Returns whether it went to the quick lane.
pub(super) fn enqueue_color_job(
    state: Arc<AppState>,
    prepared: PreparedColorJob,
    request: &StackColorRequest,
    origin: super::journal::JournaledStackJob,
    slot: Option<&str>,
) -> bool {
    let quick = takes_quick_lane(&state, &prepared);
    let spec = ColorRequest {
        job_id: prepared.public.job_id.clone(),
        database_id: prepared.public.database_id.clone(),
        project_id: prepared.public.project_id,
        request: request.clone(),
    };
    let lane = if quick {
        super::Lane::Quick
    } else {
        super::Lane::Main
    };
    submit_color(state, spec, lane, origin, prepared.public.automatic, slot);
    quick
}

/// Put a composition in the line from its request. An automatic
/// composition that a person's work stops goes back in the line as its
/// request, and its finished RC-Astro steps stay cached.
fn submit_color(
    state: Arc<AppState>,
    spec: ColorRequest,
    lane: super::Lane,
    origin: super::journal::JournaledStackJob,
    automatic: bool,
    slot: Option<&str>,
) {
    let job_id = spec.job_id.clone();
    // A composition stops while it waits for the worker, and while it runs
    // at its next channel or step; a running RC-Astro tool is killed.
    let cancel = state.stack_previews.track_job(&job_id);
    state
        .stack_previews
        .remember_origin(&job_id, origin.clone());
    // A server stop waits for a running composition to reach its next stop
    // point; the journal brings it back, and its finished RC-Astro steps
    // stay cached.
    let work = state.shutdown.hold(format!(
        "color composition for project {} (db={})",
        spec.project_id, spec.database_id
    ));
    let (failed_job, stopped_job) = (job_id.clone(), job_id.clone());
    let again = (spec.clone(), origin);
    let submission = super::Submission::<ColorWork>::new(
        "a color composition",
        &spec,
        move |state, composed| {
            if let Err(error) = composed {
                state.stack_previews.update_color(&failed_job, |job| {
                    job.state = StackJobState::Failed;
                    job.phase = "Color worker failed".into();
                    job.error = Some(format!("Color worker failed: {error}"));
                });
            }
        },
        move |state| {
            state.stack_previews.update_color(&stopped_job, |job| {
                job.state = StackJobState::Cancelled;
                job.phase = "Stopped before composing".into();
            });
        },
    )
    .map(|submission| {
        let (job_id, cancel) = (job_id.clone(), cancel.clone());
        // A stop that lands as its turn comes still keeps it from starting.
        let start_stop = cancel.clone();
        submission
            .starting_with(move |_| !start_stop.requested())
            .finally(move |state, ending| {
                if let crate::server::job_line::Ending::Panicked(error) = ending {
                    state.stack_previews.update_color(&job_id, |job| {
                        job.state = StackJobState::Failed;
                        job.phase = "Color worker failed".into();
                        job.error = Some(format!("Color worker panicked: {error}"));
                    });
                }
                state.stack_previews.forget_job_of(&job_id, &cancel);
                drop(work);
                if state.stack_previews.line.take_preempted(&job_id)
                    && !state.shutdown.is_stopping()
                    && requeue_stopped_color(state, &job_id)
                {
                    let (spec, mut origin) = again;
                    // A person may have adopted it while it stopped.
                    let automatic = state.stack_previews.is_automatic(&job_id);
                    origin.set_automatic(automatic);
                    submit_color(Arc::clone(state), spec, lane, origin, automatic, None);
                    if automatic {
                        state.stack_previews.line.move_ahead_of_automatic(&job_id);
                    }
                }
            })
    });
    match submission {
        Ok(submission) => {
            super::submit(
                &state,
                &job_id,
                lane,
                slot,
                cancel,
                Vec::new(),
                automatic,
                Box::new(submission),
            );
        }
        Err(error) => {
            state.stack_previews.update_color(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(error);
            });
            state.stack_previews.forget_job_of(&job_id, &cancel);
        }
    }
}

/// Make a composition that a person's work stopped queued again. False
/// when it did not stop, such as one that finished before the stop
/// reached it: it is not queued again.
fn requeue_stopped_color(state: &AppState, job_id: &str) -> bool {
    let mut requeued = false;
    state.stack_previews.update_color(job_id, |job| {
        if job.state != StackJobState::Cancelled {
            return;
        }
        job.state = StackJobState::Queued;
        job.error = None;
        job.phase = "Queued".into();
        requeued = true;
    });
    requeued
}

/// A color composition on the line.
struct ColorWork;

impl super::LineWork for ColorWork {
    type Spec = ColorRequest;
    type Output = ();
    /// It reads this server's catalog to prepare the composition at its
    /// turn, reads the channel stacks from the stack cache, records its
    /// progress on the composition, and writes the color index itself.
    const NEEDS: super::Needs = super::Needs::ThisServer;

    fn execute(spec: ColorRequest, worker: &super::Worker<'_>) {
        let state = worker.server();
        if let Some(prepared) = prepare_color_at_turn(&spec, state, worker.stop_flag()) {
            run_color_job(state, prepared, worker.stop_flag().clone());
        }
    }
}

/// A composition in the line: its request, prepared at its turn.
#[derive(Clone, Serialize, Deserialize)]
struct ColorRequest {
    /// The id the composition was listed under when it was previewed.
    job_id: String,
    database_id: String,
    project_id: i32,
    request: StackColorRequest,
}

/// Prepare a composition again from its request, from the channel stacks
/// as they are now. Returns the prepared composition when it is the one
/// that was previewed. When its sources changed while it waited, the
/// request becomes another composition: that one is queued, or a matching
/// finished one is reused, and the previewed record points to it. A
/// composition that cannot be prepared fails with the reason.
fn prepare_color_at_turn(
    spec: &ColorRequest,
    state: &Arc<AppState>,
    stop: &Stop,
) -> Option<PreparedColorJob> {
    let fail = |error: String| {
        state.stack_previews.update_color(&spec.job_id, |job| {
            job.state = StackJobState::Failed;
            job.phase = "Could not be prepared".into();
            job.error = Some(error);
        });
    };
    let Some(ctx) = state.get_database(&spec.database_id) else {
        fail("The database was removed while the composition waited".into());
        return None;
    };
    let prepared = match prepare_color_job(&ctx, &state.astrometry, spec.project_id, &spec.request)
    {
        Ok(prepared) => prepared,
        Err(error) => {
            fail(error.message());
            return None;
        }
    };
    if prepared.public.job_id == spec.job_id {
        return Some(prepared);
    }
    // Stopped while it was read, or the server is stopping: no other
    // composition takes its place. A preempted one goes back in the line
    // as its request.
    if stop.requested() || state.shutdown.is_stopping() {
        state.stack_previews.update_color(&spec.job_id, |job| {
            job.state = StackJobState::Cancelled;
            job.phase = "Stopped before composing".into();
        });
        return None;
    }
    let automatic = state.stack_previews.is_automatic(&spec.job_id);
    let placement = super::build_set::Placement {
        automatic,
        ..Default::default()
    };
    match start_prepared_color(
        state,
        &ctx,
        spec.project_id,
        &spec.request,
        prepared,
        placement,
    ) {
        Ok(became) => {
            tracing::info!(
                job_id = spec.job_id,
                became = became.job_id,
                "A queued composition's sources changed while it waited; it runs as a new composition"
            );
            state.stack_previews.update_color(&spec.job_id, |job| {
                job.state = StackJobState::Replaced;
                job.replaced_by = vec![became.job_id.clone()];
                job.phase = "Queued".into();
            });
            // It keeps the turn the previewed composition had: a person's
            // next, an automatic one ahead of the other automatic work.
            if automatic {
                state
                    .stack_previews
                    .line
                    .move_ahead_of_automatic(&became.job_id);
            } else {
                state.stack_previews.move_in_line(&became.job_id, 0);
            }
        }
        Err(error) => fail(error.message()),
    }
    None
}

/// The error a composition ends with when it is stopped, read back so the
/// card says it stopped rather than that it failed.
const COLOR_STOPPED: &str = "Stopped";

/// Where a running composition may stop: between channels and steps.
struct ColorStop {
    cancel: Stop,
}

impl ColorStop {
    fn stopped(&self) -> bool {
        self.cancel.requested()
    }

    fn check(&self) -> Result<(), String> {
        if self.stopped() {
            Err(COLOR_STOPPED.into())
        } else {
            Ok(())
        }
    }

    /// Kills a running RC-Astro tool when the composition is stopped.
    fn signal(&self) -> seiza_stacking::CancelSignal {
        seiza_stacking::CancelSignal::from(self.cancel.clone())
    }
}

fn run_color_job(state: &Arc<AppState>, prepared: PreparedColorJob, cancel: Stop) {
    let stop = ColorStop { cancel };
    let job_id = prepared.public.job_id.clone();
    state.stack_previews.update_color(&job_id, |job| {
        job.state = StackJobState::Running;
        job.phase = "Loading channel stacks".into();
    });
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        // A mono rebuild ahead in the shared queue may have replaced a
        // selected revision while this color job waited for the permit.
        let ctx = state
            .get_database(&prepared.public.database_id)
            .ok_or_else(|| "The color source database is no longer available".to_string())?;
        let latest = load_latest_stacks(&ctx, prepared.public.project_id)
            .map_err(|error| format!("Failed to validate color sources: {error:?}"))?;
        validate_current_color_sources(&prepared.public, &latest)?;
        compose_color(
            state,
            &prepared.public,
            &prepared.background_regions,
            &prepared.stack_root,
            &prepared.rc_astro_schemas,
            &prepared.rc_astro_chains,
            prepared.color_calibration.as_ref(),
            &stop,
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
                // A composition that ran beside a build of its own target can
                // finish after that build replaced one of its channels. The
                // newer color follows that build, so this one is kept but
                // does not take the card back.
                let superseded = state
                    .get_database(&completed.database_id)
                    .and_then(|ctx| load_latest_stacks(&ctx, completed.project_id).ok())
                    .is_some_and(|latest| {
                        validate_current_color_sources(&completed, &latest).is_err()
                    });
                if superseded {
                    tracing::info!(
                        job_id,
                        "Color composition finished after a newer channel stack; the card keeps the newer one"
                    );
                }
                let persisted =
                    persist_color_manifest(&prepared.stack_root, &completed).and_then(|()| {
                        if superseded {
                            Ok(())
                        } else {
                            state
                                .stack_previews
                                .persist_latest_color(&prepared.stack_root, &completed)
                        }
                    });
                match persisted {
                    Ok(()) => {
                        let _ = state.stack_previews.insert_color(completed);
                        state.stack_previews.prune_cache(&prepared.stack_root);
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
        // A stop kills a running tool, whose own complaint is not the
        // reason: the stop is.
        Ok(Err(_)) if stop.stopped() => {
            let phase = if state.shutdown.is_stopping() {
                "Stopped for the server stop; it resumes after the restart"
            } else {
                "Stopped"
            };
            state.stack_previews.update_color(&job_id, |job| {
                job.state = StackJobState::Cancelled;
                job.phase = phase.into();
                job.progress.active_phase = None;
                job.progress.partial_units = 0.0;
            });
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

/// Whether `fit` already carries the surface `model` names.
fn fit_uses_model(fit: &BackgroundFit, model: &ModelConfig) -> bool {
    match (&fit.model, model) {
        (
            FittedModel::Polynomial { degree, .. },
            ModelConfig::Polynomial { degree: wanted, .. },
        ) => degree == wanted,
        (FittedModel::RadialBasis { .. }, ModelConfig::RadialBasis { .. }) => true,
        _ => false,
    }
}

/// Refit one channel with the model chosen for every channel.
///
/// The refit keeps whatever catalog protection the first fit ended up with,
/// and keeps its candidate scores so the manifest still shows why the shared
/// model won.
fn refit_channel_background(
    image: &LinearImage,
    extraction: &StackBackgroundExtraction,
    protected_regions: Vec<ProtectedRegion>,
    first: &BackgroundFit,
    model: &ModelConfig,
) -> Result<BackgroundFit, seiza_background::Error> {
    let mut config = extraction.config.clone();
    config.model = model.clone();
    config.protected_regions = if first.diagnostics.protected_regions > 0 {
        protected_regions
    } else {
        Vec::new()
    };
    let mut fit = seiza_background::fit_background(
        &image.data,
        image.width,
        image.height,
        image.channels,
        &config,
    )?;
    if let Some(mut selection) = first.diagnostics.model_selection.clone() {
        selection.selected = match &fit.model {
            FittedModel::Polynomial { degree, .. } => format!("polynomial_{degree}"),
            _ => "radial_basis".into(),
        };
        fit.diagnostics.model_selection = Some(selection);
    }
    Ok(fit)
}

#[allow(clippy::too_many_arguments)]
fn compose_color(
    state: &Arc<AppState>,
    job: &StackColorJob,
    background_regions: &BTreeMap<StackColorRole, Vec<ProtectedRegion>>,
    stack_root: &FsPath,
    rc_astro_schemas: &BTreeMap<StackColorRole, Vec<(String, seiza_stacking::ExternalToolSchema)>>,
    rc_astro_chains: &BTreeMap<StackColorRole, Vec<String>>,
    color_calibration: Option<&ColorCalibrationPlan>,
    stop: &ColorStop,
) -> Result<(), String> {
    let progress = ColorProgressTracker {
        state,
        job_id: &job.job_id,
    };
    let reference_role = reference_role(job.kind);
    let reference_source = job
        .sources
        .iter()
        .find(|source| source.role == reference_role)
        .ok_or_else(|| "Color job has no reference channel".to_string())?;
    let cached_inputs = job.linear_input_id.as_deref().and_then(|input_id| {
        load_cached_color_input_manifest(
            stack_root,
            input_id,
            reference_role,
            &job.sources,
            &job.input_rc_astro_ids,
        )
    });
    let reused_inputs = cached_inputs.is_some();
    let (
        mut reference,
        mut resolved_backgrounds,
        mut background_protection_fallbacks,
        mut resolved_deconvolutions,
        mut resolved_rc_astro,
        mut registered_transforms,
    ) = if let Some((manifest, reference)) = cached_inputs {
        state.stack_previews.update_color(&job.job_id, |current| {
            current.processed_channels = current.total_channels;
            current.resolved_backgrounds = manifest.resolved_backgrounds.clone();
            current.background_protection_fallbacks =
                manifest.background_protection_fallbacks.clone();
            current.resolved_input_deconvolutions = manifest.resolved_deconvolutions.clone();
            current.resolved_input_rc_astro = manifest.resolved_rc_astro.clone();
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
            manifest.resolved_rc_astro,
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
        let reference = load_source_frame(stack_root, reference_source)?;
        progress.advance(StackColorProgressPhase::LoadingSources, 1);
        (
            reference,
            BTreeMap::new(),
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
        + if job.processing.as_ref().is_some_and(|processing| {
            !processing.input_deconvolutions.is_empty() || !processing.input_rc_astro.is_empty()
        }) {
            COLOR_DECONVOLUTION_BYTES_PER_PIXEL
        } else {
            0
        }
        + if color_calibration.is_some() {
            COLOR_CALIBRATION_BYTES_PER_PIXEL
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
    let priority = super::job_priority(state.stack_previews.is_automatic(&job.job_id));
    let budget = crate::concurrency::plan_workers(None, &policy, priority, Some(pixels));
    let lease = state.lease_workers(priority, budget.workers);
    let pool = crate::concurrency::ComputePool::take("stack-color", lease.workers)
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Stack color {}: {} worker(s) — {}; {}",
        job.job_id,
        lease.workers,
        budget.rationale,
        lease.summary()
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
                stack_root,
                input_id,
                source.role,
            ))
            .map_err(|error| error.to_string())?;
            validate_mono(&frame.image, source.role)?;
            frame
        } else {
            stop.check()?;
            progress.begin(
                StackColorProgressPhase::LoadingSources,
                format!("Loading {} stack", source.role.label()),
                Some(source.role),
                None,
            );
            let frame = load_source_frame(stack_root, source)?;
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
            let mut fitted = Vec::with_capacity(job.sources.len());
            for source in &job.sources {
                let frame = if source.role == reference_role {
                    &reference
                } else {
                    frames
                        .get(&source.role)
                        .ok_or_else(|| format!("{} source was not loaded", source.role.label()))?
                };
                stop.check()?;
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
                fitted.push((source.role, fit, fallback));
            }

            // Automatic selection runs per channel. Left alone it can curve
            // one channel and leave another flat, which tints the corners.
            let fits: Vec<&BackgroundFit> = fitted.iter().map(|(_, fit, _)| fit).collect();
            let shared = if fits.len() > 1 {
                seiza_background::select_shared_model(&extraction.config.model, &fits)
            } else {
                None
            };

            for (role, mut fit, fallback) in fitted {
                let frame = if role == reference_role {
                    &mut reference
                } else {
                    frames
                        .get_mut(&role)
                        .ok_or_else(|| format!("{} source was not loaded", role.label()))?
                };
                if let Some(shared) = shared.as_ref()
                    && !fit_uses_model(&fit, shared)
                {
                    progress.begin(
                        StackColorProgressPhase::BackgroundPreparation,
                        format!("Refitting {} background to match the other channels", role.label()),
                        Some(role),
                        None,
                    );
                    fit = refit_channel_background(
                        &frame.image,
                        extraction,
                        background_regions.get(&role).cloned().unwrap_or_default(),
                        &fit,
                        shared,
                    )
                    .map_err(|error| {
                        format!("Failed to refit {} background: {error}", role.label())
                    })?;
                }
                progress.begin(
                    StackColorProgressPhase::BackgroundPreparation,
                    format!("Correcting {} background", role.label()),
                    Some(role),
                    None,
                );
                fit.correct_in_place_with_strength(
                    &mut frame.image.data,
                    extraction.correction_mode,
                    extraction.strength,
                )
                .map_err(|error| format!("Failed to correct {} background: {error}", role.label()))?;
                progress.advance(StackColorProgressPhase::BackgroundPreparation, 1);
                state.stack_previews.update_color(&job.job_id, |current| {
                    current.resolved_backgrounds.insert(role, fit.clone());
                    if let Some(reason) = fallback.as_ref() {
                        current
                            .background_protection_fallbacks
                            .insert(role, reason.clone());
                    }
                });
                if let Some(reason) = fallback {
                    background_protection_fallbacks.insert(role, reason);
                }
                resolved_backgrounds.insert(role, fit);
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

    let mut source_headers = frames
        .iter()
        .map(|(role, frame)| (*role, frame.headers.clone()))
        .collect::<BTreeMap<_, _>>();
    source_headers.insert(reference_role, reference.headers.clone());
    pool.install(|| {
        let reference_headers = reference.headers;
        let reference_image = reference.image;
        // What color calibration did, and the stretch every composed channel
        // shares when calibration waits for the composition.
        let mut calibration: Option<StackColorCalibrationOutcome> = None;
        let mut deferred_stages: Option<Vec<super::stretch::StackStretchRequest>> = None;
        let open_catalog = || {
            state
                .astrometry
                .gaia_photometry_catalog()
                .map(|(catalog, _)| catalog)
        };
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
                stop.check()?;
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
                    StackColorProgressPhase::RcAstroInputs,
                    "Reused RC-Astro input processing",
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
                        stop.check()?;
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

                if processing.input_rc_astro.is_empty() {
                    progress.skip(
                        StackColorProgressPhase::RcAstroInputs,
                        "RC-Astro input processing skipped (disabled)",
                    );
                } else {
                    for source in &job.sources {
                        let Some(config) = processing.input_rc_astro.get(&source.role) else {
                            continue;
                        };
                        let image = images.get(&source.role).ok_or_else(|| {
                            format!("{} registered image is missing", source.role.label())
                        })?;
                        let chain = rc_astro_chains
                            .get(&source.role)
                            .ok_or_else(|| "RC-Astro chain identity is missing".to_string())?;
                        let schemas = rc_astro_schemas
                            .get(&source.role)
                            .ok_or_else(|| "RC-Astro tool schemas are missing".to_string())?;
                        let headers = registered_channel_headers(
                            &reference_headers,
                            &source_headers[&source.role],
                            &source.filter_name,
                        );
                        let steps = config.steps.len();
                        let outcome = super::rc_astro::apply_rc_astro(
                            stack_root,
                            chain,
                            config,
                            schemas,
                            image,
                            &headers,
                            &mut |label, fraction| {
                                let percent = (fraction.clamp(0.0, 1.0) * 100.0).round() as u8;
                                progress.begin(
                                    StackColorProgressPhase::RcAstroInputs,
                                    format!("{}: {label} ({percent}%)", source.role.label()),
                                    Some(source.role),
                                    None,
                                );
                                progress.partial(steps, fraction);
                            },
                            // A stop kills the tool in hand; the steps it
                            // finished stay cached for the next composition.
                            Some(&stop.signal()),
                            &mut || stop.check(),
                        )?;
                        let id = chain
                            .last()
                            .ok_or_else(|| "RC-Astro chain is empty".to_string())?;
                        let result = StackColorRcAstroResult {
                            fits_url: format!(
                                "/api/db/{}/stack-previews/rc-astro/{id}/fits",
                                job.database_id
                            ),
                            stars_fits_url: outcome.result.has_stars.then(|| {
                                format!(
                                    "/api/db/{}/stack-previews/rc-astro/{id}/fits?stars=true",
                                    job.database_id
                                )
                            }),
                            result: outcome.result,
                        };
                        images.insert(source.role, outcome.image);
                        resolved_rc_astro.insert(source.role, result.clone());
                        state.stack_previews.update_color(&job.job_id, |current| {
                            current.resolved_input_rc_astro.insert(source.role, result);
                        });
                        progress.advance(StackColorProgressPhase::RcAstroInputs, steps);
                    }
                    progress.finish(StackColorProgressPhase::RcAstroInputs);
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
                        resolved_rc_astro: resolved_rc_astro.clone(),
                    };
                    store_cached_color_inputs(
                        stack_root,
                        input_id,
                        &job.sources,
                        &images,
                        &reference_headers,
                        &manifest,
                    )?;
                }
            }

            stop.check()?;
            // Calibration needs one stretch for every channel, after it:
            // three stretches of their own pull each channel to the same
            // median and undo it. When the composed channels share their
            // stages, as they do by default, they are composed linear and
            // stretched together after calibration.
            let ready = color_calibration.and_then(|plan| {
                plan.placement
                    .as_ref()
                    .ok()
                    .map(|placement| (plan, placement))
            });
            match color_calibration {
                None => progress.skip(
                    StackColorProgressPhase::CalibratingColor,
                    "Color calibration skipped (off)",
                ),
                Some(plan) => {
                    if let Err(skip) = &plan.placement {
                        calibration = Some(StackColorCalibrationOutcome::Skipped(skip.clone()));
                    }
                }
            }
            deferred_stages = ready.and_then(|_| {
                shared_input_stages(processing, &composed_roles(job.kind, job.palette))
            });
            // LRGB, and channels with stretches of their own, are
            // calibrated now, before luminance joins or anything stretches.
            if let Some((plan, placement)) = ready
                && (job.kind == StackColorKind::Lrgb || deferred_stages.is_none())
            {
                progress.begin(
                    StackColorProgressPhase::CalibratingColor,
                    "Calibrating color against Gaia",
                    None,
                    None,
                );
                calibration = Some(calibrate_registered_channels(
                    placement,
                    &plan.settings,
                    &mut images,
                    &open_catalog,
                ));
            }
            // Calibrated channels stretched one by one keep little of the
            // balance; the card says so.
            if deferred_stages.is_none()
                && processing
                    .input_stretches
                    .values()
                    .any(|stages| !stages.is_empty())
                && let Some(StackColorCalibrationOutcome::Calibrated(report)) = &mut calibration
            {
                report.stretched_separately = true;
            }
            if let Some(outcome) = &calibration {
                report_calibration(&progress, outcome);
            }

            let input_stage_count = processing
                .input_stretches
                .values()
                .map(Vec::len)
                .sum::<usize>();
            if deferred_stages.is_some() {
                progress.skip(
                    StackColorProgressPhase::StretchingInputs,
                    "Channel stretches run on the calibrated composition",
                );
            } else if input_stage_count == 0 {
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
                    let output = apply_input_stretch_stack(
                        &stack,
                        &image.data,
                        super::stretch::image_shape(&image),
                        |event| {
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
                        },
                    )
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
                input_transfer: if deferred_stages.is_some() {
                    ColorTransfer::LinearLight
                } else {
                    ColorTransfer::DisplayReferred
                },
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
                StackColorProgressPhase::CalibratingColor,
                "Color calibration skipped (legacy quick look)",
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

        stop.check()?;
        progress.begin(
            StackColorProgressPhase::ComposingColor,
            format!("Composing {}", job.label),
            None,
            None,
        );
        let composed_roles = composed_roles(job.kind, job.palette);
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

        if let (Some(processing), Some(channel_stages), Some((plan, placement))) = (
            &job.processing,
            deferred_stages.as_ref(),
            color_calibration.and_then(|plan| {
                plan.placement
                    .as_ref()
                    .ok()
                    .map(|placement| (plan, placement))
            }),
        ) {
            stop.check()?;
            progress.begin(
                StackColorProgressPhase::StretchingOutput,
                "Stretching the composition",
                None,
                None,
            );
            let stretched = match calibration.as_ref() {
                // LRGB: calibrated before luminance joined.
                Some(outcome) => {
                    let linked = outcome.is_calibrated();
                    let (image, plans) = match composition_stretch(
                        channel_stages,
                        &processing.output_stretches,
                        linked,
                    )? {
                        Some(stack) => stretch_linear_composition(&composition.image, &stack)?,
                        None => (composition.image.clone(), Vec::new()),
                    };
                    StretchedComposition {
                        image,
                        calibration: None,
                        plans,
                    }
                }
                None => {
                    progress.begin(
                        StackColorProgressPhase::CalibratingColor,
                        "Calibrating color against Gaia",
                        None,
                        None,
                    );
                    let image = std::mem::replace(
                        &mut composition.image,
                        LinearImage::new(1, 1, 3, vec![0.0; 3])
                            .map_err(|error| error.to_string())?,
                    );
                    calibrate_and_stretch_composition(
                        placement,
                        &plan.settings,
                        image,
                        composition.region,
                        channel_stages,
                        &processing.output_stretches,
                        &open_catalog,
                    )?
                }
            };
            if let Some(outcome) = stretched.calibration {
                report_calibration(&progress, &outcome);
                calibration = Some(outcome);
            }
            let (channel_plans, output_plans) = stretched
                .plans
                .split_at(channel_stages.len().min(stretched.plans.len()));
            let (channel_plans, output_plans) = (channel_plans.to_vec(), output_plans.to_vec());
            let stretched_any = !stretched.plans.is_empty();
            state.stack_previews.update_color(&job.job_id, |current| {
                for role in &composed_roles {
                    current
                        .resolved_input_stretches
                        .insert(*role, channel_plans.clone());
                }
                current.resolved_output_stretches = output_plans;
            });
            composition = ColorComposition {
                image: stretched.image,
                transfer: if stretched_any {
                    ColorTransfer::DisplayReferred
                } else {
                    ColorTransfer::LinearLight
                },
                ..composition
            };
            progress.finish(StackColorProgressPhase::StretchingOutput);
        } else if let Some(processing) = &job.processing {
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
                    .apply_f32_image_with_progress(
                        &composition.image.data,
                        super::stretch::image_shape(&composition.image),
                        |event| {
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
                        },
                    )
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
        let mut fits_headers_cards = Vec::new();
        let fits_destination = color_fits_path(stack_root, &job.job_id);
        let parent = fits_destination
            .parent()
            .ok_or_else(|| "Color FITS path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = fits_destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        if let Some(outcome) = calibration {
            fits_headers_cards = color_calibration_cards(&outcome);
            state.stack_previews.update_color(&job.job_id, |current| {
                current.color_calibration = Some(outcome);
            });
        }
        let mut fits_headers = reference_headers.clone();
        fits_headers.extend(fits_headers_cards);
        write_color_fits_f32(&temporary, &composition, &fits_headers, &job.label)
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
            &color_preview_path(stack_root, &job.job_id),
            &color_original_preview_path(stack_root, &job.job_id),
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

const RGB_ROLES: [StackColorRole; 3] = [
    StackColorRole::Red,
    StackColorRole::Green,
    StackColorRole::Blue,
];

/// The Gaia photometry catalog Seiza reads, or why it cannot.
type OpenCatalog<'a> = &'a dyn Fn() -> Result<Arc<seiza::catalog::PhotometryCatalog>, String>;

/// Calibrate the registered R, G and B channels in place, on the grid every
/// channel is registered to, before anything stretches them: for LRGB,
/// whose luminance joins after, and for channels with stretches of their
/// own. A calibration that cannot run leaves them as they were.
fn calibrate_registered_channels(
    placement: &color_calibration::StarPlacement,
    settings: &StackColorCalibration,
    images: &mut BTreeMap<StackColorRole, LinearImage>,
    open_catalog: OpenCatalog<'_>,
) -> StackColorCalibrationOutcome {
    use color_calibration::StackColorCalibrationSkipReason as Reason;

    let grid = images
        .get(&StackColorRole::Red)
        .map(|image| (image.width, image.height));
    if grid
        != Some((
            placement.orientation.output_width,
            placement.orientation.output_height,
        ))
    {
        return StackColorCalibrationOutcome::skipped(
            Reason::Failed,
            "The reference stack's sky layout does not match its pixels; rebuild it",
        );
    }
    let wcs = match placement.wcs_on(None) {
        Ok(wcs) => wcs,
        Err(error) => return StackColorCalibrationOutcome::skipped(Reason::Failed, error),
    };
    let catalog = match open_catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            return StackColorCalibrationOutcome::skipped(
                Reason::NoGaiaCatalog,
                format!("Gaia colors could not be opened: {error}"),
            )
        }
    };
    let [Some(mut red), Some(mut green), Some(mut blue)] =
        RGB_ROLES.map(|role| images.remove(&role))
    else {
        return StackColorCalibrationOutcome::skipped(
            Reason::Failed,
            "A red, green or blue channel is missing",
        );
    };
    let outcome = color_calibration::calibrate_channels(
        [&mut red, &mut green, &mut blue],
        &wcs,
        &catalog,
        settings,
    );
    images.insert(StackColorRole::Red, red);
    images.insert(StackColorRole::Green, green);
    images.insert(StackColorRole::Blue, blue);
    outcome
}

/// A composition Seiza calibrated and stretched, or stretched alone.
struct StretchedComposition {
    image: LinearImage,
    /// What calibration did, when it ran on this composition.
    calibration: Option<StackColorCalibrationOutcome>,
    /// Each stage's resolved plan, input stages first.
    plans: Vec<serde_json::Value>,
}

/// The shared channel stretch, then the output stretch, as one stack.
/// When `linked` is false the channel stages run per channel, as each
/// channel's own stretch would have; the output stages keep their own
/// strategy.
fn composition_stretch(
    channel_stages: &[super::stretch::StackStretchRequest],
    output_stages: &[super::stretch::StackStretchRequest],
    linked: bool,
) -> Result<Option<StretchStack>, String> {
    let configs = channel_stages
        .iter()
        .map(|stage| {
            let mut config = stage.config();
            if !linked {
                config.color_strategy = seiza_stretch::ColorStrategy::Unlinked;
            }
            config
        })
        .chain(
            output_stages
                .iter()
                .map(super::stretch::StackStretchRequest::config),
        )
        .collect::<Vec<_>>();
    if configs.is_empty() {
        return Ok(None);
    }
    StretchStack::new(configs)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn stretch_plans(plans: &[seiza_stretch::StretchPlan]) -> Result<Vec<serde_json::Value>, String> {
    plans
        .iter()
        .map(|plan| serde_json::to_value(plan).map_err(|error| error.to_string()))
        .collect()
}

fn stretch_linear_composition(
    image: &LinearImage,
    stack: &StretchStack,
) -> Result<(LinearImage, Vec<serde_json::Value>), String> {
    let output = stack
        .apply_f32_image(&image.data, super::stretch::image_shape(image))
        .map_err(|error| error.to_string())?;
    let plans = stretch_plans(&output.plans)?;
    let image = LinearImage::new(image.width, image.height, image.channels, output.data)
        .map_err(|error| error.to_string())?;
    Ok((image, plans))
}

/// Calibrate a linear RGB composition against Gaia and stretch it, in
/// Seiza's one step: the reference frame's solve is carried through the
/// stack's layout and the composition's crop. The channel stages go to
/// Seiza unlinked: once it has calibrated, Seiza links them so the balance
/// survives, and when it cannot calibrate they stretch each channel on its
/// own, as they would have without calibration. One linked stretch of
/// uncalibrated channels would show the camera's color cast.
fn calibrate_and_stretch_composition(
    placement: &color_calibration::StarPlacement,
    settings: &StackColorCalibration,
    image: LinearImage,
    crop: ReferenceRegion,
    channel_stages: &[super::stretch::StackStretchRequest],
    output_stages: &[super::stretch::StackStretchRequest],
    open_catalog: OpenCatalog<'_>,
) -> Result<StretchedComposition, String> {
    use color_calibration::StackColorCalibrationSkipReason as Reason;

    let stack = composition_stretch(channel_stages, output_stages, false)?;
    let uncalibrated = |image: LinearImage, outcome: StackColorCalibrationOutcome| {
        let (image, plans) = match &stack {
            Some(stack) => stretch_linear_composition(&image, stack)?,
            None => (image, Vec::new()),
        };
        Ok::<_, String>(StretchedComposition {
            image,
            calibration: Some(outcome),
            plans,
        })
    };
    let wcs = match placement.wcs_on(Some(crop)) {
        Ok(wcs) => wcs,
        Err(error) => {
            return uncalibrated(
                image,
                StackColorCalibrationOutcome::skipped(Reason::Failed, error),
            )
        }
    };
    let catalog = match open_catalog() {
        Ok(catalog) => catalog,
        Err(error) => {
            return uncalibrated(
                image,
                StackColorCalibrationOutcome::skipped(
                    Reason::NoGaiaCatalog,
                    format!("Gaia colors could not be opened: {error}"),
                ),
            )
        }
    };
    let gaia = seiza_stacking::GaiaColors::Catalog(&catalog);
    let Some(stack) = stack else {
        let mut image = image;
        let outcome = seiza_stacking::calibrate_color_image(
            &mut image,
            Some(&wcs),
            gaia,
            &settings.options(),
        );
        return Ok(StretchedComposition {
            image,
            calibration: Some(outcome.into()),
            plans: Vec::new(),
        });
    };
    let result =
        seiza_stacking::calibrate_and_stretch(image, Some(&wcs), gaia, &settings.options(), &stack)
            .map_err(|error| error.to_string())?;
    Ok(StretchedComposition {
        image: result.image,
        calibration: Some(result.calibration.into()),
        plans: stretch_plans(&result.stretch_plans)?,
    })
}

/// Report a calibration's outcome on the job's progress.
fn report_calibration(progress: &ColorProgressTracker<'_>, outcome: &StackColorCalibrationOutcome) {
    match outcome {
        StackColorCalibrationOutcome::Calibrated(report) => {
            tracing::info!(
                gains = ?report.gains,
                offsets = ?report.offsets,
                stars = report.stars_used,
                stretched_separately = report.stretched_separately,
                "color calibrated against Gaia"
            );
            if report.stretched_separately {
                progress.begin(
                    StackColorProgressPhase::CalibratingColor,
                    "Calibrated, then each channel stretched separately",
                    None,
                    None,
                );
            }
            progress.finish(StackColorProgressPhase::CalibratingColor);
        }
        StackColorCalibrationOutcome::Skipped(skip) => {
            tracing::info!(reason = ?skip.reason, detail = %skip.detail, "color calibration skipped");
            progress.skip(
                StackColorProgressPhase::CalibratingColor,
                format!("Not color calibrated: {}", skip.detail),
            );
        }
    }
}

/// The input stretch every composed channel shares, when they share one.
/// Calibration then runs on the linear composition, and that stretch after
/// it, once for all the channels.
fn shared_input_stages(
    processing: &StackColorProcessing,
    roles: &[StackColorRole],
) -> Option<Vec<super::stretch::StackStretchRequest>> {
    let mut stages = roles.iter().map(|role| {
        serde_json::to_value(
            processing
                .input_stretches
                .get(role)
                .cloned()
                .unwrap_or_default(),
        )
        .ok()
    });
    let first = stages.next()??;
    stages
        .all(|stages| stages.as_ref() == Some(&first))
        .then(|| serde_json::from_value(first).ok())
        .flatten()
}

/// The channels a composition takes, in the order it takes them, which is
/// the order its coverage report keeps.
fn composed_roles(
    kind: StackColorKind,
    palette: Option<StackNarrowbandPalette>,
) -> Vec<StackColorRole> {
    match kind {
        StackColorKind::Rgb => RGB_ROLES.to_vec(),
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
            if palette.is_some_and(|palette| palette.seiza().requires_sii()) {
                roles.push(StackColorRole::Sii);
            }
            roles
        }
    }
}

/// FITS cards recording a calibration, as `seiza color-calibrate` writes
/// them.
fn color_calibration_cards(
    outcome: &StackColorCalibrationOutcome,
) -> Vec<(String, seiza_fits::HeaderValue)> {
    use seiza_fits::HeaderValue;

    let StackColorCalibrationOutcome::Calibrated(report) = outcome else {
        return Vec::new();
    };
    vec![
        (
            "COLORCAL".into(),
            HeaderValue::String("Gaia DR3 BP-RP".into()),
        ),
        (
            "CCWHITE".into(),
            HeaderValue::Float(f64::from(report.white_bp_rp)),
        ),
        (
            "CCGAINR".into(),
            HeaderValue::Float(f64::from(report.gains[0])),
        ),
        (
            "CCGAING".into(),
            HeaderValue::Float(f64::from(report.gains[1])),
        ),
        (
            "CCGAINB".into(),
            HeaderValue::Float(f64::from(report.gains[2])),
        ),
        (
            "CCSTARS".into(),
            HeaderValue::Integer(report.stars_used as i64),
        ),
    ]
}

fn registered_channel_headers(
    reference: &[(String, seiza_fits::HeaderValue)],
    source: &[(String, seiza_fits::HeaderValue)],
    filter_name: &str,
) -> Vec<(String, seiza_fits::HeaderValue)> {
    // Registered pixels use the reference grid but retain their own channel's
    // acquisition metadata, never the reference channel's filter or exposure.
    let wcs_key = |key: &str| {
        matches!(
            key,
            "CRPIX1"
                | "CRPIX2"
                | "CRVAL1"
                | "CRVAL2"
                | "CTYPE1"
                | "CTYPE2"
                | "CUNIT1"
                | "CUNIT2"
                | "CDELT1"
                | "CDELT2"
                | "CROTA1"
                | "CROTA2"
                | "WCSAXES"
                | "RADESYS"
                | "EQUINOX"
                | "LONPOLE"
                | "LATPOLE"
                | "SKYORIEN"
        ) || [
            "CD1_", "CD2_", "PC1_", "PC2_", "PV1_", "PV2_", "A_", "B_", "AP_", "BP_",
        ]
        .iter()
        .any(|prefix| key.starts_with(prefix))
    };
    let mut headers = source
        .iter()
        .filter(|(key, _)| !wcs_key(key) && key != "FILTER")
        .cloned()
        .collect::<Vec<_>>();
    headers.extend(reference.iter().filter(|(key, _)| wcs_key(key)).cloned());
    headers.push((
        "FILTER".into(),
        seiza_fits::HeaderValue::String(filter_name.into()),
    ));
    headers
}

fn load_source_frame(stack_root: &FsPath, source: &StackColorSource) -> Result<FitsFrame, String> {
    let frame = crate::image_io::open_linear_frame(super::fits_path(
        stack_root,
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

/// A composition's prepared inputs, when they are cached whole: the manifest
/// reads, matches this version, these roles and RC-Astro results, and every
/// channel's FITS is there. Reads no pixels.
fn cached_color_inputs(
    stack_root: &FsPath,
    input_id: &str,
    sources: &[StackColorSource],
    rc_astro_ids: &BTreeMap<StackColorRole, String>,
) -> Option<CachedColorInputs> {
    let bytes = std::fs::read(color_input_manifest_path(stack_root, input_id)).ok()?;
    let manifest = serde_json::from_slice::<CachedColorInputs>(&bytes).ok()?;
    let roles = sources.iter().map(|source| source.role).collect::<Vec<_>>();
    if manifest.schema_version != COLOR_INPUT_CACHE_VERSION
        || manifest.input_id != input_id
        || manifest.roles != roles
        || !rc_astro_artifacts_exist(stack_root, rc_astro_ids, &manifest.resolved_rc_astro)
    {
        return None;
    }
    roles
        .iter()
        .all(|role| color_input_fits_path(stack_root, input_id, *role).is_file())
        .then_some(manifest)
}

fn load_cached_color_input_manifest(
    stack_root: &FsPath,
    input_id: &str,
    reference_role: StackColorRole,
    sources: &[StackColorSource],
    rc_astro_ids: &BTreeMap<StackColorRole, String>,
) -> Option<(CachedColorInputs, FitsFrame)> {
    let manifest = cached_color_inputs(stack_root, input_id, sources, rc_astro_ids)?;
    let reference = crate::image_io::open_linear_frame(color_input_fits_path(
        stack_root,
        input_id,
        reference_role,
    ))
    .ok()?;
    validate_mono(&reference.image, reference_role).ok()?;
    Some((manifest, reference))
}

fn rc_astro_artifacts_exist(
    stack_root: &FsPath,
    ids: &BTreeMap<StackColorRole, String>,
    results: &BTreeMap<StackColorRole, StackColorRcAstroResult>,
) -> bool {
    ids.iter().all(|(role, id)| {
        super::validate_job_id(id).is_ok()
            && super::rc_astro::rc_astro_fits_path(stack_root, id).is_file()
            && results.get(role).is_some_and(|result| {
                !result.result.has_stars
                    || super::rc_astro::rc_astro_stars_path(stack_root, id).is_file()
            })
    })
}

pub(super) fn color_job_artifacts_exist(stack_root: &FsPath, job: &StackColorJob) -> bool {
    color_artifacts_exist(stack_root, &job.job_id)
        && rc_astro_artifacts_exist(
            stack_root,
            &job.input_rc_astro_ids,
            &job.resolved_input_rc_astro,
        )
}

fn store_cached_color_inputs(
    stack_root: &FsPath,
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
        let destination = color_input_fits_path(stack_root, input_id, source.role);
        let parent = destination
            .parent()
            .ok_or_else(|| "Color input FITS path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = destination.with_extension(format!("{}.tmp.fits", temporary_tag()));
        write_processed_image_fits_f32(&temporary, image, reference_headers, &[])
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &destination).map_err(|error| error.to_string())?;
    }
    write_json_atomic(&color_input_manifest_path(stack_root, input_id), manifest)
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
    match std::fs::read(super::latest_path(&ctx.stack_root, project_id)) {
        Ok(bytes) => {
            let latest: LatestStackPreviews = serde_json::from_slice(&bytes).map_err(|error| {
                AppError::InternalError(format!("Invalid latest stack preview index: {error}"))
            })?;
            if latest.database_id != ctx.id || latest.project_id != project_id {
                return Err(AppError::NotFound);
            }
            let mut latest = super::current_project_latest_stacks(ctx, project_id, latest)?;
            // WBPP's stacks are channel sources too.
            latest
                .groups
                .extend(super::wbpp_stacks::load_index(ctx, project_id).groups);
            Ok(latest)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(LatestStackPreviews {
            schema_version: 1,
            database_id: ctx.id.clone(),
            project_id,
            updated_unix_seconds: 0,
            groups: super::wbpp_stacks::load_index(ctx, project_id).groups,
        }),
        Err(error) => Err(AppError::InternalError(format!(
            "Failed to read latest stack preview index: {error}"
        ))),
    }
}

pub(super) fn load_latest_colors(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
) -> Result<LatestStackColorPreviews, AppError> {
    match std::fs::read(latest_color_path(&ctx.stack_root, project_id)) {
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

pub(super) fn retained_calibration_source(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
    job_id: &str,
    revision: &str,
) -> Result<Option<StackColorJob>, AppError> {
    Ok(load_latest_colors(ctx, project_id)?
        .jobs
        .into_iter()
        .find(|job| {
            job.database_id == ctx.id
                && job.project_id == project_id
                && job.job_id == job_id
                && job.artifact_revision == revision
                && job.state == StackJobState::Completed
        }))
}

/// The color previews to compose for a target once a build leaves none of
/// its channels waiting.
///
/// Every preview composed before for the target comes back with the newest
/// stack of each of its channels: the same role, filter, exposure group and
/// origin (PSF Guard or WBPP), so a choice the person made between families
/// holds. One whose channels did not change is left alone. A target with no
/// preview yet gets its first one when `defaults` carries the person's
/// display pipeline: LRGB when it has L, R, G and B, else RGB, else the
/// first narrowband palette its channels allow, from PSF Guard's own stacks.
pub(super) fn requests_after_build(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
    target_id: i32,
    defaults: Option<&StackColorProcessing>,
) -> Result<Vec<StackColorRequest>, AppError> {
    let latest = load_latest_stacks(ctx, project_id)?;
    let sources = collect_sources(&ctx.stack_root, &latest);
    let Some(target) = sources.get(&target_id) else {
        return Ok(Vec::new());
    };
    let remembered = load_latest_colors(ctx, project_id)?
        .jobs
        .into_iter()
        .filter(|job| job.target_id == target_id)
        .collect::<Vec<_>>();
    Ok(plan_after_build(target_id, target, &remembered, defaults))
}

/// What makes two channel stacks the same channel of a composition: the
/// filter, whether WBPP made it, and the exposure group. A rebuild of a
/// channel is a newer stack of the same family.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SourceFamily {
    filter_name: String,
    wbpp: bool,
    exposure_group: Option<String>,
}

impl SourceFamily {
    fn of(source: &StackColorSource) -> Self {
        Self {
            filter_name: source.filter_name.clone(),
            wbpp: source.wbpp,
            exposure_group: source
                .exposure_group
                .as_ref()
                .map(|group| group.key.clone()),
        }
    }

    fn holds(&self, source: &StackColorSource) -> bool {
        *self == Self::of(source)
    }
}

/// The family of each stack a request names, as the target lists them now.
/// A request that names none, or a stack the target no longer lists, has no
/// family for that role.
pub(super) fn named_families(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
    request: &StackColorRequest,
) -> BTreeMap<StackColorRole, SourceFamily> {
    if request.input_sources.is_empty() {
        return BTreeMap::new();
    }
    let Ok(latest) = load_latest_stacks(ctx, project_id) else {
        return BTreeMap::new();
    };
    families_in(&ctx.stack_root, &latest, request)
}

/// [`named_families`] in a latest index already read.
fn families_in(
    stack_root: &FsPath,
    latest: &LatestStackPreviews,
    request: &StackColorRequest,
) -> BTreeMap<StackColorRole, SourceFamily> {
    let sources = collect_sources(stack_root, latest);
    let Some(target) = sources.get(&request.target_id) else {
        return BTreeMap::new();
    };
    request
        .input_sources
        .iter()
        .filter_map(|(role, named)| {
            target
                .by_role
                .get(role)?
                .iter()
                .find(|source| {
                    source.job_id == named.job_id
                        && source.group_index == named.group_index
                        && source.artifact_revision == named.artifact_revision
                })
                .map(|source| (*role, SourceFamily::of(source)))
        })
        .collect()
}

/// The request with each stack it names moved to the newest stack of the
/// same family, as [`plan_after_build`] moves a finished composition: a
/// composition asked for while its channels were building means the stacks
/// those builds made. A role whose family has no stack now keeps what it
/// named, and preparing it says that stack is no longer current.
pub(super) fn with_newest_sources(
    ctx: &crate::server::database_context::DatabaseContext,
    project_id: i32,
    request: &StackColorRequest,
    families: &BTreeMap<StackColorRole, SourceFamily>,
) -> StackColorRequest {
    if families.is_empty() {
        return request.clone();
    }
    let Ok(latest) = load_latest_stacks(ctx, project_id) else {
        return request.clone();
    };
    newest_sources_in(&ctx.stack_root, &latest, request, families)
}

/// [`with_newest_sources`] in a latest index already read.
fn newest_sources_in(
    stack_root: &FsPath,
    latest: &LatestStackPreviews,
    request: &StackColorRequest,
    families: &BTreeMap<StackColorRole, SourceFamily>,
) -> StackColorRequest {
    let mut request = request.clone();
    let sources = collect_sources(stack_root, latest);
    let Some(target) = sources.get(&request.target_id) else {
        return request;
    };
    for (role, family) in families {
        if let Some(newest) = target
            .by_role
            .get(role)
            .and_then(|candidates| candidates.iter().find(|source| family.holds(source)))
        {
            request.input_sources.insert(
                *role,
                StackColorSourceRef {
                    job_id: newest.job_id.clone(),
                    group_index: newest.group_index,
                    artifact_revision: newest.artifact_revision.clone(),
                },
            );
        }
    }
    request
}

/// [`requests_after_build`] once the target's channel stacks and its
/// remembered color previews are in hand.
fn plan_after_build(
    target_id: i32,
    target: &TargetSources,
    remembered: &[StackColorJob],
    defaults: Option<&StackColorProcessing>,
) -> Vec<StackColorRequest> {
    let reference = |source: &StackColorSource| StackColorSourceRef {
        job_id: source.job_id.clone(),
        group_index: source.group_index,
        artifact_revision: source.artifact_revision.clone(),
    };
    let mut requests = Vec::new();
    for previous in remembered {
        let mut input_sources = BTreeMap::new();
        let mut changed = false;
        for source in &previous.sources {
            let newest = target.by_role.get(&source.role).and_then(|candidates| {
                candidates.iter().find(|candidate| {
                    candidate.filter_name == source.filter_name
                        && candidate.wbpp == source.wbpp
                        && candidate.exposure_group.as_ref().map(|group| &group.key)
                            == source.exposure_group.as_ref().map(|group| &group.key)
                })
            });
            let chosen = match newest {
                Some(candidate) => {
                    changed |= candidate.job_id != source.job_id
                        || candidate.group_index != source.group_index
                        || candidate.artifact_revision != source.artifact_revision;
                    reference(candidate)
                }
                None => reference(source),
            };
            input_sources.insert(source.role, chosen);
        }
        if changed {
            requests.push(StackColorRequest {
                target_id,
                kind: previous.kind,
                palette: previous.palette,
                force: false,
                crop: previous.crop,
                processing: previous.processing.clone(),
                input_sources,
            });
        }
    }
    if let (true, Some(defaults)) = (remembered.is_empty(), defaults)
        && let Some(request) = first_composite(target_id, target, defaults)
    {
        requests.push(request);
    }
    requests
}

/// A target's first color preview: the richest kind its channels make, each
/// role from its one PSF Guard stack, with the person's display pipeline
/// narrowed to those roles. `None` when no kind has exactly one stack for
/// every role it needs.
fn first_composite(
    target_id: i32,
    target: &TargetSources,
    defaults: &StackColorProcessing,
) -> Option<StackColorRequest> {
    let own = |role: StackColorRole| -> Option<&StackColorSource> {
        let candidates = target
            .by_role
            .get(&role)?
            .iter()
            .filter(|candidate| !candidate.wbpp)
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [only] => Some(*only),
            _ => None,
        }
    };
    let has = |role: StackColorRole| own(role).is_some();
    let mut kinds = vec![(StackColorKind::Lrgb, None), (StackColorKind::Rgb, None)];
    if has(StackColorRole::Ha) && has(StackColorRole::Oiii) {
        kinds.extend(
            StackNarrowbandPalette::all(has(StackColorRole::Sii))
                .into_iter()
                .map(|palette| (StackColorKind::Narrowband, Some(palette))),
        );
    }
    kinds.into_iter().find_map(|(kind, palette)| {
        let roles = required_roles(kind, palette);
        let input_sources = roles
            .iter()
            .map(|&role| {
                own(role).map(|source| {
                    (
                        role,
                        StackColorSourceRef {
                            job_id: source.job_id.clone(),
                            group_index: source.group_index,
                            artifact_revision: source.artifact_revision.clone(),
                        },
                    )
                })
            })
            .collect::<Option<BTreeMap<_, _>>>()?;
        let mut processing = defaults.clone();
        processing
            .input_stretches
            .retain(|role, _| roles.contains(role));
        processing
            .input_deconvolutions
            .retain(|role, _| roles.contains(role));
        processing
            .input_rc_astro
            .retain(|role, _| roles.contains(role));
        Some(StackColorRequest {
            target_id,
            kind,
            palette,
            force: false,
            crop: StackColorCrop::default(),
            processing: Some(processing),
            input_sources,
        })
    })
}

fn collect_sources(
    stack_root: &FsPath,
    latest: &LatestStackPreviews,
) -> BTreeMap<i32, TargetSources> {
    let mut targets = BTreeMap::<i32, TargetSources>::new();
    for entry in &latest.groups {
        if super::validate_job_id(&entry.job_id).is_err()
            || entry.group.state != super::StackGroupState::Ready
            || entry.group.output_channels != 1
        {
            continue;
        }
        if !super::fits_path(stack_root, &entry.job_id, entry.group.index).is_file() {
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
                label: match (&entry.wbpp, &entry.group.exposure_group) {
                    // Two masters of one filter differ only in sub-exposure,
                    // so the label carries it where WBPP's file name did.
                    (Some(wbpp), _) => match wbpp.exposure_seconds {
                        Some(seconds) => {
                            format!("{} (WBPP, {seconds} s)", entry.group.filter_name)
                        }
                        None => format!("{} (WBPP)", entry.group.filter_name),
                    },
                    (None, Some(group)) => format!("{} ({})", entry.group.filter_name, group.label),
                    (None, None) => entry.group.filter_name.clone(),
                },
                exposure_group: entry.group.exposure_group.clone(),
                job_id: entry.job_id.clone(),
                group_index: entry.group.index,
                artifact_revision: entry.artifact_revision.clone(),
                accepted_frames: entry.group.accepted_frames,
                wbpp: entry.wbpp.is_some(),
                total_exposure_seconds: Some(entry.group.total_exposure_seconds),
                reference_image_id: entry.group.reference_image_id,
                sky_orientation: entry.group.sky_orientation.clone(),
                registration_transform: None,
            });
    }
    for target in targets.values_mut() {
        target.unmapped_filters.sort();
        target.unmapped_filters.dedup();
        for candidates in target.by_role.values_mut() {
            candidates.sort_by(|left, right| {
                left.filter_name.cmp(&right.filter_name).then_with(|| {
                    left.exposure_group
                        .as_ref()
                        .map(|group| &group.key)
                        .cmp(&right.exposure_group.as_ref().map(|group| &group.key))
                })
            });
        }
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
            let present = target.by_role.keys().copied().collect::<HashSet<_>>();
            let rgb_available = [
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue,
            ]
            .iter()
            .all(|role| present.contains(role));
            let lrgb_available = rgb_available && present.contains(&StackColorRole::Luminance);
            let has_ha_oiii =
                present.contains(&StackColorRole::Ha) && present.contains(&StackColorRole::Oiii);
            let narrowband_palettes = if has_ha_oiii {
                StackNarrowbandPalette::all(present.contains(&StackColorRole::Sii))
            } else {
                Vec::new()
            };
            StackColorTargetAvailability {
                target_id: *target_id,
                target_name: target.target_name.clone(),
                available_roles,
                source_candidates: target.by_role.values().flatten().cloned().collect(),
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

fn source_is_current(
    source: &StackColorSource,
    target_id: i32,
    latest: &LatestStackPreviews,
) -> bool {
    latest.groups.iter().any(|entry| {
        entry.job_id == source.job_id
            && entry.artifact_revision == source.artifact_revision
            && entry.group.index == source.group_index
            && entry.group.target_id == target_id
            && entry.group.filter_name == source.filter_name
            && entry.group.exposure_group == source.exposure_group
            && classify_filter(&entry.group.filter_name) == Some(source.role)
    })
}

fn validate_current_color_sources(
    job: &StackColorJob,
    latest: &LatestStackPreviews,
) -> Result<(), String> {
    if latest.database_id != job.database_id || latest.project_id != job.project_id {
        return Err("The color source project changed while the build was queued".into());
    }
    if !job
        .sources
        .iter()
        .all(|source| source_is_current(source, job.target_id, latest))
    {
        return Err("A selected channel stack changed while the color build was queued; refresh the channel selection and build again".into());
    }
    Ok(())
}

fn color_artifacts_exist(stack_root: &FsPath, job_id: &str) -> bool {
    color_preview_path(stack_root, job_id).is_file()
        && color_original_preview_path(stack_root, job_id).is_file()
        && color_fits_path(stack_root, job_id).is_file()
}

fn color_job_outdated_reason(
    ctx: &crate::server::database_context::DatabaseContext,
    astrometry: &crate::astrometry::AstrometryContext,
    job: &StackColorJob,
    latest: &LatestStackPreviews,
) -> Result<Option<String>, AppError> {
    if job.cache_version != STACK_COLOR_CACHE_VERSION
        || job.stacking_version != SEIZA_STACKING_OUTPUT_VERSION
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
        source_is_current(source, job.target_id, latest)
            && super::fits_path(&ctx.stack_root, &source.job_id, source.group_index).is_file()
    }) {
        return Ok(Some("one or more source channel stacks changed".into()));
    }
    if job
        .processing
        .as_ref()
        .is_some_and(|processing| processing.input_processing == ColorInputProcessing::Channels)
        && channel_card_processing(&ctx.stack_root, &job.sources)?
            != job.followed_channel_processing
    {
        return Ok(Some(
            "a channel card's deconvolution or RC-Astro steps changed".into(),
        ));
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
    if let (Some(basis), Some(settings)) = (
        job.color_calibration_basis.as_ref(),
        job.processing
            .as_ref()
            .and_then(|processing| processing.color_calibration),
    ) {
        let current = plan_color_calibration(ctx, astrometry, job.kind, &job.sources, settings);
        if &current.basis != basis {
            return Ok(Some(
                "the plate solve or Gaia colors for color calibration changed".into(),
            ));
        }
    }
    if !color_job_artifacts_exist(&ctx.stack_root, job) {
        return Ok(Some("a cached color artifact is missing".into()));
    }
    Ok(None)
}

fn persist_color_manifest(stack_root: &FsPath, job: &StackColorJob) -> Result<(), String> {
    write_json_atomic(&color_manifest_path(stack_root, &job.job_id), job)
}

fn persist_latest_color(stack_root: &FsPath, job: &StackColorJob) -> Result<(), String> {
    let path = latest_color_path(stack_root, job.project_id);
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
            && existing.source_family_key == job.source_family_key
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
            && job.stacking_version == SEIZA_STACKING_OUTPUT_VERSION
    });
    latest
}

/// A temporary file's tag, unique to this write, so a composition in the
/// quick lane and one in the line never write the same temporary file.
fn temporary_tag() -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let next = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{}-{next}", std::process::id())
}

fn write_json_atomic(path: &FsPath, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Color manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", temporary_tag()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn color_dir(stack_root: &FsPath, job_id: &str) -> PathBuf {
    crate::server::storage::stack_folder(stack_root, crate::server::storage::stack_kind::COLOR)
        .join(job_id)
}

pub(super) fn color_manifest_path(stack_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(stack_root, job_id).join("manifest.json")
}

fn color_preview_path(stack_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(stack_root, job_id).join("preview.png")
}

pub(super) fn color_original_preview_path(stack_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(stack_root, job_id).join("preview-original.png")
}

fn color_fits_path(stack_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(stack_root, job_id).join("color.fits")
}

fn color_input_dir(stack_root: &FsPath, input_id: &str) -> PathBuf {
    crate::server::storage::stack_folder(
        stack_root,
        crate::server::storage::stack_kind::COLOR_INPUTS,
    )
    .join(input_id)
}

fn color_input_manifest_path(stack_root: &FsPath, input_id: &str) -> PathBuf {
    color_input_dir(stack_root, input_id).join("manifest.json")
}

fn color_input_fits_path(stack_root: &FsPath, input_id: &str, role: StackColorRole) -> PathBuf {
    color_input_dir(stack_root, input_id).join(format!("{}.fits", role_cache_name(role)))
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

pub(super) fn latest_color_rc_astro_ids(stack_root: &FsPath) -> Vec<String> {
    super::read_latest_indices::<LatestStackColorPreviews>(&crate::server::storage::stack_folder(
        stack_root,
        crate::server::storage::stack_kind::COLOR,
    ))
    .into_iter()
    .flat_map(|latest| latest.jobs)
    .flat_map(|job| job.input_rc_astro_ids.into_values())
    .collect()
}

fn latest_color_path(stack_root: &FsPath, project_id: i32) -> PathBuf {
    crate::server::storage::stack_folder(stack_root, crate::server::storage::stack_kind::COLOR)
        .join(format!("latest-project-{project_id}.json"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::stack_preview::{
        LatestStackPreviewGroup, StackGroupState, StackGroupStatus, StackSkyOrientation,
    };

    #[test]
    fn registered_rc_astro_inputs_keep_their_own_filter_and_reference_grid() {
        use seiza_fits::HeaderValue::{Float, String as Text};
        let reference = vec![
            ("FILTER".into(), Text("Ha".into())),
            ("EXPTIME".into(), Float(300.0)),
            ("DATE-OBS".into(), Text("2026-01-01".into())),
            ("CRPIX1".into(), Float(10.0)),
            ("CRPIX2".into(), Float(20.0)),
            ("CRVAL1".into(), Float(30.0)),
            ("CRVAL2".into(), Float(40.0)),
            ("CTYPE1".into(), Text("RA---TAN".into())),
            ("CTYPE2".into(), Text("DEC--TAN".into())),
            ("CD1_1".into(), Float(0.001)),
        ];
        let source = vec![
            ("FILTER".into(), Text("OIII".into())),
            ("EXPTIME".into(), Float(600.0)),
            ("DATE-OBS".into(), Text("2026-01-02".into())),
            ("CRPIX1".into(), Float(99.0)),
            ("PC1_1".into(), Float(9.0)),
        ];
        let headers = registered_channel_headers(&reference, &source, "OIII");
        let cache = tempfile::tempdir().unwrap();
        let path = cache.path().join("processed.fits");
        write_processed_image_fits_f32(
            &path,
            &LinearImage::new(2, 2, 1, vec![10.0; 4]).unwrap(),
            &headers,
            &[],
        )
        .unwrap();
        let image = crate::image_io::open_linear_frame(&path).unwrap();
        let header = |key: &str| {
            image
                .headers
                .iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value)
        };
        assert_eq!(header("FILTER"), Some(&Text("OIII".into())));
        assert_eq!(header("EXPTIME"), Some(&Float(600.0)));
        assert_eq!(header("DATE-OBS"), Some(&Text("2026-01-02".into())));
        assert_eq!(header("CRPIX1"), Some(&Float(10.0)));
        assert_eq!(header("CD1_1"), Some(&Float(0.001)));
        assert!(header("PC1_1").is_none());
    }

    #[test]
    fn color_rc_astro_validation_rejects_foreign_roles_and_keeps_processing_identity() {
        let request: StackColorRequest = serde_json::from_value(serde_json::json!({
            "target_id":1,"kind":"rgb","processing":{"input_rc_astro":{"ha":{"steps":[{"tool":"bxt"}]}}}
        })).unwrap();
        assert!(validate_request(&request).is_err());
        let first = BTreeMap::from([(StackColorRole::Red, vec!["bxt-v1".into(), "nxt-v1".into()])]);
        let changed =
            BTreeMap::from([(StackColorRole::Red, vec!["bxt-v1".into(), "nxt-v2".into()])]);
        assert_ne!(
            rc_astro_input_cache_id("input", &first).unwrap(),
            rc_astro_input_cache_id("input", &changed).unwrap()
        );
        assert_ne!(
            rc_astro_input_cache_id("input", &first).unwrap(),
            rc_astro_input_cache_id("different-background", &first).unwrap()
        );
    }

    #[test]
    fn a_channel_rc_astro_input_keys_on_what_changes_its_pixels() {
        let narrowband = |oiii: &str| {
            vec![
                channel(StackColorRole::Ha, "Ha", "ha-1", false),
                channel(StackColorRole::Oiii, "OIII", oiii, false),
                channel(StackColorRole::Sii, "SII", "sii-1", false),
            ]
        };
        let plain = StackColorProcessing::default();
        let id = |role, processing: &StackColorProcessing, sources: &[StackColorSource]| {
            color_role_input_id(
                "db",
                1,
                7,
                role,
                StackColorRole::Ha,
                processing,
                sources,
                &BTreeMap::new(),
            )
            .unwrap()
        };
        let before = narrowband("oiii-1");
        let after = narrowband("oiii-2");
        // New OIII frames: SII registers onto the same H-alpha from the same
        // stack, so its RC-Astro chain is reused; OIII's reruns.
        assert_eq!(
            id(StackColorRole::Sii, &plain, &before),
            id(StackColorRole::Sii, &plain, &after)
        );
        assert_eq!(
            id(StackColorRole::Ha, &plain, &before),
            id(StackColorRole::Ha, &plain, &after)
        );
        assert_ne!(
            id(StackColorRole::Oiii, &plain, &before),
            id(StackColorRole::Oiii, &plain, &after)
        );
        // A new reference moves every channel's registered pixels.
        let mut new_reference = narrowband("oiii-1");
        new_reference[0] = channel(StackColorRole::Ha, "Ha", "ha-2", false);
        assert_ne!(
            id(StackColorRole::Sii, &plain, &before),
            id(StackColorRole::Sii, &plain, &new_reference)
        );
        // Another channel's deconvolution leaves this one alone; its own
        // does not.
        let mut deconvolved = StackColorProcessing::default();
        deconvolved.input_deconvolutions.insert(
            StackColorRole::Oiii,
            seiza_deconvolution::DeconvolutionConfig::conservative(2.5),
        );
        assert_eq!(
            id(StackColorRole::Sii, &plain, &before),
            id(StackColorRole::Sii, &deconvolved, &before)
        );
        assert_ne!(
            id(StackColorRole::Oiii, &plain, &before),
            id(StackColorRole::Oiii, &deconvolved, &before)
        );
        // Background extraction fits one model for every channel, so any
        // channel's new frames move all of them.
        let extracted: StackColorProcessing = serde_json::from_value(serde_json::json!({
            "background_extraction": {"config": {}, "protect_catalog_emission": false},
            "input_stretches": {},
            "output_stretches": []
        }))
        .unwrap();
        assert_ne!(
            id(StackColorRole::Sii, &extracted, &before),
            id(StackColorRole::Sii, &extracted, &after)
        );
    }

    #[test]
    fn the_job_line_hears_what_a_running_color_job_does_and_how_far_its_tool_got() {
        let mut job = running_color_job(&"c".repeat(64), StackJobState::Running);
        job.phase = "H-alpha: BlurXTerminator (42%)".into();
        job.progress.completed_units = 5;
        job.progress.total_units = 20;
        job.progress.partial_units = 0.84;
        let entry = super::super::color_activity(&job);
        assert_eq!(
            entry.progress_label.as_deref(),
            Some("H-alpha: BlurXTerminator (42%)")
        );
        assert_eq!((entry.processed_units, entry.total_units), (584, 2000));
        // Between phases it still says what it is, never a bare count.
        job.phase.clear();
        assert_eq!(
            super::super::color_activity(&job).progress_label.as_deref(),
            Some("Composing color")
        );
        // Queued, it says nothing of its own; the line says it waits.
        job.state = StackJobState::Queued;
        assert_eq!(super::super::color_activity(&job).progress_label, None);
    }

    #[test]
    fn a_stopped_color_composition_stops_at_its_next_check() {
        let stop = ColorStop {
            cancel: Stop::new(),
        };
        assert!(stop.check().is_ok());
        stop.cancel.request();
        assert_eq!(stop.check(), Err(COLOR_STOPPED.to_string()));
    }

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
            order: crate::server::stack_preview::snr::StackFrameOrder::Capture,
            scoring: crate::server::stack_preview::StackScoringSettings::default(),
            method: Default::default(),
            wbpp: None,
            created_unix_seconds: 10,
            cache_version: super::super::STACK_PREVIEW_CACHE_VERSION,
            group: StackGroupStatus {
                snr: None,
                snr_url: None,
                final_pass: None,
                calibration_progress: None,
                index,
                target_id: 7,
                target_name: "Color target".into(),
                filter_name: filter_name.into(),
                exposure_group: None,
                state: StackGroupState::Ready,
                phase: "ready".into(),
                total_candidates: 3,
                eligible_frames: 3,
                quality_excluded: 0,
                missing_files: 0,
                calibration_excluded: 0,
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
                color_calibration: None,
                color_calibration_basis: None,
                color_calibration_outdated: None,
                preview_stretch_version: None,
                preview_revision: None,
                preview_render_failed: None,
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

    /// A composition about to queue: `deconvolve` asks for deconvolution of
    /// the red channel, `rc_astro` gives it an RC-Astro chain, and
    /// `linear_input_id` names its prepared inputs.
    fn prepared_color(
        stack_root: &FsPath,
        job_id: &str,
        deconvolve: bool,
        rc_astro: bool,
        linear_input_id: Option<&str>,
    ) -> PreparedColorJob {
        let mut public = running_color_job(job_id, StackJobState::Queued);
        public.phase = "Waiting for color processor".into();
        public.linear_input_id = linear_input_id.map(str::to_string);
        if deconvolve {
            let mut processing = StackColorProcessing::default();
            processing.input_deconvolutions.insert(
                StackColorRole::Red,
                seiza_deconvolution::DeconvolutionConfig::conservative(3.1),
            );
            public.processing = Some(processing);
        }
        let mut rc_astro_chains = BTreeMap::new();
        if rc_astro {
            rc_astro_chains.insert(StackColorRole::Red, vec!["blurxterminator".to_string()]);
        }
        PreparedColorJob {
            public,
            stack_root: stack_root.to_path_buf(),
            background_regions: BTreeMap::new(),
            rc_astro_schemas: BTreeMap::new(),
            rc_astro_chains,
            color_calibration: None,
        }
    }

    /// A whole cache of prepared inputs for a composition with no channels.
    fn write_cached_inputs(root: &FsPath, input_id: &str) {
        let cached = CachedColorInputs {
            schema_version: COLOR_INPUT_CACHE_VERSION,
            input_id: input_id.into(),
            roles: Vec::new(),
            registered_transforms: BTreeMap::new(),
            resolved_backgrounds: BTreeMap::new(),
            background_protection_fallbacks: BTreeMap::new(),
            resolved_deconvolutions: BTreeMap::new(),
            resolved_rc_astro: BTreeMap::new(),
        };
        write_json_atomic(&color_input_manifest_path(root, input_id), &cached).unwrap();
    }

    #[test]
    fn a_composition_runs_quickly_unless_it_prepares_inputs_at_length() {
        let cache = tempfile::tempdir().unwrap();
        let root = cache.path();
        assert!(prepared_color(root, "plain", false, false, Some("inputs")).runs_quickly());
        assert!(prepared_color(root, "legacy", false, false, None).runs_quickly());
        assert!(!prepared_color(root, "deconvolve", true, false, Some("inputs")).runs_quickly());
        assert!(!prepared_color(root, "rc-astro", false, true, Some("inputs")).runs_quickly());
        // A manifest that does not read as this version's cache is no cache.
        let manifest = color_input_manifest_path(root, "inputs");
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, b"{}").unwrap();
        assert!(!prepared_color(root, "deconvolve", true, false, Some("inputs")).runs_quickly());
        // Once those inputs are prepared and cached whole, reusing them is quick.
        write_cached_inputs(root, "inputs");
        assert!(prepared_color(root, "deconvolve", true, false, Some("inputs")).runs_quickly());
        assert!(prepared_color(root, "rc-astro", false, true, Some("inputs")).runs_quickly());
    }

    #[test]
    fn a_running_quick_composition_is_not_named_as_holding_the_worker() {
        let manager = super::super::StackPreviewManager::new();
        let running = running_color_job(&"c".repeat(64), StackJobState::Running);
        assert!(manager.insert_color(running.clone()));
        assert_eq!(
            manager.worker_holder("other").as_deref(),
            Some("the color composition of Color target · SHO")
        );
        // In the quick lane's line, it does not hold the worker.
        let quick = crate::server::job_line::testing::entry(
            &running.job_id,
            super::super::Lane::Quick,
            crate::concurrency::Stop::new(),
        );
        assert!(manager.line.submit(quick, None).is_ok());
        assert_eq!(manager.worker_holder("other"), None);
        manager.line.remove(&running.job_id);
        assert!(manager.worker_holder("other").is_some());
    }

    #[test]
    fn the_quick_lane_is_for_a_persons_composition_whose_inputs_no_one_else_prepares() {
        let cache = tempfile::tempdir().unwrap();
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        let quick = prepared_color(cache.path(), &"a".repeat(64), false, false, Some("inputs"));
        assert!(takes_quick_lane(&state, &quick));
        // An automatic composition follows its builds in the line.
        let mut automatic =
            prepared_color(cache.path(), &"b".repeat(64), false, false, Some("inputs"));
        automatic.public.automatic = true;
        assert!(!takes_quick_lane(&state, &automatic));
        // Another composition preparing the same inputs sends it to the line,
        // so the two never write them at once.
        assert!(state.stack_previews.insert_color(automatic.public.clone()));
        assert!(!takes_quick_lane(&state, &quick));
        // Once those inputs are cached, it only reads them.
        write_cached_inputs(cache.path(), "inputs");
        assert!(takes_quick_lane(&state, &quick));
    }

    #[test]
    fn a_composition_that_cannot_be_prepared_at_its_turn_fails_with_why() {
        let cache = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let previewed = prepared_color(cache.path(), &"e".repeat(64), false, false, None);
        assert!(state.stack_previews.insert_color(previewed.public.clone()));
        let spec = ColorRequest {
            job_id: previewed.public.job_id.clone(),
            database_id: "removed".into(),
            project_id: previewed.public.project_id,
            request: test_request(),
        };
        assert!(prepare_color_at_turn(&spec, &state, &Stop::new()).is_none());
        let job = state.stack_previews.get_color(&spec.job_id).unwrap();
        assert_eq!(job.state, StackJobState::Failed);
        assert_eq!(job.phase, "Could not be prepared");
        assert_eq!(
            job.error.as_deref(),
            Some("The database was removed while the composition waited")
        );
    }

    fn test_request() -> StackColorRequest {
        serde_json::from_value(serde_json::json!({"target_id": 1, "kind": "rgb"})).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_composition_stopped_while_it_waits_for_the_quick_lane_ends_there() {
        let cache = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        // Another composition holds the quick lane.
        let busy = state.stack_previews.line.try_reserve_quick().unwrap();
        let waiting = prepared_color(cache.path(), &"d".repeat(64), false, false, Some("inputs"));
        let job_id = waiting.public.job_id.clone();
        assert!(state.stack_previews.insert_color(waiting.public.clone()));
        let origin = super::super::processing_queue::test_origin("waiting");
        assert!(enqueue_color_job(
            Arc::clone(&state),
            waiting,
            &test_request(),
            origin,
            None
        ));
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert_eq!(
            state.stack_previews.get_color(&job_id).unwrap().state,
            StackJobState::Queued
        );
        assert!(state.stack_previews.request_cancel(&job_id));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state.stack_previews.get_color(&job_id).unwrap().state != StackJobState::Cancelled {
            assert!(
                std::time::Instant::now() < deadline,
                "the stop never reached it"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(!state.stack_previews.is_quick(&job_id));
        drop(busy);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_quick_composition_runs_while_a_build_holds_the_worker() {
        let cache = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        // A build holds the stack worker.
        let busy = state.stack_previews.line.reserve_main().await;
        let quick = prepared_color(cache.path(), &"a".repeat(64), false, false, Some("inputs"));
        let slow = prepared_color(cache.path(), &"b".repeat(64), true, false, Some("other"));
        for prepared in [&quick, &slow] {
            assert!(state.stack_previews.insert_color(prepared.public.clone()));
        }
        let origin = |id: &str| super::super::processing_queue::test_origin(id);
        enqueue_color_job(
            Arc::clone(&state),
            quick,
            &test_request(),
            origin("quick"),
            None,
        );
        enqueue_color_job(
            Arc::clone(&state),
            slow,
            &test_request(),
            origin("slow"),
            None,
        );
        // The quick one starts at once. Its database is not configured, so it
        // ends there; what matters is that it did not wait for the build.
        let quick_id = "a".repeat(64);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while state
            .stack_previews
            .get_color(&quick_id)
            .is_some_and(|job| job.state == StackJobState::Queued)
        {
            assert!(
                std::time::Instant::now() < deadline,
                "the quick composition never started"
            );
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(state.stack_previews.line_position(&quick_id), None);
        // The deconvolving one waits in the build line.
        let slow_id = "b".repeat(64);
        assert_eq!(state.stack_previews.line_position(&slow_id), Some(0));
        assert_eq!(
            state.stack_previews.get_color(&slow_id).unwrap().state,
            StackJobState::Queued
        );
        assert!(state.stack_previews.request_cancel(&slow_id));
        drop(busy);
    }

    fn running_color_job(job_id: &str, state: StackJobState) -> StackColorJob {
        let mut progress = color_progress(3, None);
        progress.completed_units = 4;
        StackColorJob {
            automatic: false,
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
            source_family_key: String::new(),
            state,
            phase: "Registering source channels".into(),
            processed_channels: 1,
            total_channels: 3,
            progress,
            created_unix_seconds: 200,
            artifact_revision: "rev".into(),
            cache_version: STACK_COLOR_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_OUTPUT_VERSION.into(),
            background_version: SEIZA_BACKGROUND_VERSION.into(),
            deconvolution_version: String::new(),
            linear_input_id: None,
            sources: Vec::new(),
            processing: None,
            resolved_input_stretches: BTreeMap::new(),
            resolved_input_deconvolutions: BTreeMap::new(),
            resolved_input_rc_astro: BTreeMap::new(),
            input_rc_astro_ids: BTreeMap::new(),
            followed_channel_processing: BTreeMap::new(),
            resolved_output_stretches: Vec::new(),
            resolved_backgrounds: BTreeMap::new(),
            resolved_background_protection: BTreeMap::new(),
            background_protection_fallbacks: BTreeMap::new(),
            preview_url: String::new(),
            fits_url: String::new(),
            error: None,
            outdated: false,
            outdated_reason: None,
            preparing: None,
            replaced_by: Vec::new(),
            color_calibration_basis: None,
            color_calibration: None,
        }
    }

    fn channel(role: StackColorRole, filter: &str, job: &str, wbpp: bool) -> StackColorSource {
        StackColorSource {
            role,
            filter_name: filter.into(),
            label: filter.into(),
            exposure_group: None,
            job_id: job.into(),
            group_index: 0,
            artifact_revision: format!("rev-{job}"),
            accepted_frames: 5,
            wbpp,
            total_exposure_seconds: None,
            reference_image_id: None,
            sky_orientation: None,
            registration_transform: None,
        }
    }

    fn sources(channels: Vec<StackColorSource>) -> TargetSources {
        let mut by_role = BTreeMap::<StackColorRole, Vec<StackColorSource>>::new();
        for source in channels {
            by_role.entry(source.role).or_default().push(source);
        }
        TargetSources {
            target_name: "Color target".into(),
            by_role,
            unmapped_filters: Vec::new(),
        }
    }

    fn defaults() -> StackColorProcessing {
        StackColorProcessing {
            input_stretches: [
                StackColorRole::Luminance,
                StackColorRole::Red,
                StackColorRole::Green,
                StackColorRole::Blue,
                StackColorRole::Ha,
            ]
            .into_iter()
            .map(|role| (role, Vec::new()))
            .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_composed_preview_follows_its_newest_channels_and_keeps_its_family() {
        use StackColorRole::{Blue, Green, Red};
        let mut previous = running_color_job("old-rgb", StackJobState::Completed);
        previous.kind = StackColorKind::Rgb;
        previous.palette = None;
        previous.target_id = 7;
        previous.sources = vec![
            channel(Red, "R", "old-r", false),
            channel(Green, "G", "g", false),
            channel(Blue, "B", "b", false),
        ];
        // R was stacked again; WBPP also made an R, which is another family.
        let target = sources(vec![
            channel(Red, "R", "new-r", false),
            channel(Red, "R", "wbpp-r", true),
            channel(Green, "G", "g", false),
            channel(Blue, "B", "b", false),
        ]);
        let requests = plan_after_build(
            7,
            &target,
            std::slice::from_ref(&previous),
            Some(&defaults()),
        );
        assert_eq!(
            requests.len(),
            1,
            "only the remembered preview, no first composite"
        );
        let request = &requests[0];
        assert_eq!(request.kind, StackColorKind::Rgb);
        assert_eq!(request.input_sources[&Red].job_id, "new-r");
        assert_eq!(request.input_sources[&Green].job_id, "g");

        // Nothing changed: nothing to compose.
        previous.sources[0] = channel(Red, "R", "new-r", false);
        assert!(plan_after_build(7, &target, &[previous], Some(&defaults())).is_empty());
    }

    fn deconvolution(fwhm: f32) -> seiza_deconvolution::DeconvolutionConfig {
        seiza_deconvolution::DeconvolutionConfig::conservative(fwhm)
    }

    fn rc_astro_chain() -> super::super::rc_astro::RcAstroProcessing {
        serde_json::from_value(serde_json::json!({
            "steps": [{"tool": "bxt", "parameters": {"sharpen_stars": 0.25}}]
        }))
        .unwrap()
    }

    #[test]
    fn processing_saved_before_the_choice_keeps_its_own_settings() {
        // A record from an earlier release has no `input_processing`.
        let saved: StackColorProcessing = serde_json::from_value(serde_json::json!({
            "background_extraction": null,
            "input_deconvolutions": {"red": deconvolution(2.5)},
            "input_stretches": {},
            "output_stretches": [],
        }))
        .unwrap();
        assert_eq!(saved.input_processing, ColorInputProcessing::Own);
        assert!(saved
            .input_deconvolutions
            .contains_key(&StackColorRole::Red));
        // Written back, it is the record it was: its job id is unchanged.
        let written = serde_json::to_value(&saved).unwrap();
        assert!(written.get("input_processing").is_none(), "{written}");

        let following = StackColorProcessing {
            input_processing: ColorInputProcessing::Channels,
            ..saved
        };
        assert_eq!(
            serde_json::to_value(&following).unwrap()["input_processing"],
            "channels"
        );
    }

    #[test]
    fn following_the_cards_replaces_each_inputs_own_tools() {
        use StackColorRole::{Blue, Green, Red};
        let mut processing = StackColorProcessing {
            input_processing: ColorInputProcessing::Channels,
            input_deconvolutions: BTreeMap::from([(Blue, deconvolution(4.0))]),
            input_rc_astro: BTreeMap::from([(Red, rc_astro_chain())]),
            ..defaults()
        };
        let followed = BTreeMap::from([
            (
                Red,
                FollowedChannelProcessing {
                    deconvolution: Some(deconvolution(2.5)),
                    rc_astro: None,
                },
            ),
            (
                Green,
                FollowedChannelProcessing {
                    deconvolution: None,
                    rc_astro: Some(rc_astro_chain()),
                },
            ),
            (Blue, FollowedChannelProcessing::default()),
        ]);
        follow_channel_cards(&mut processing, &followed);
        assert_eq!(
            processing.input_deconvolutions,
            BTreeMap::from([(Red, deconvolution(2.5))]),
            "Blue's card applied nothing, so its own deconvolution goes too"
        );
        assert_eq!(
            processing.input_rc_astro,
            BTreeMap::from([(Green, rc_astro_chain())]),
            "Red's card has no RC-Astro steps, so its own chain goes"
        );
    }

    #[test]
    fn the_job_id_follows_the_tools_and_whether_they_come_from_the_cards() {
        let sources = vec![
            channel(StackColorRole::Red, "R", "red", false),
            channel(StackColorRole::Green, "G", "green", false),
            channel(StackColorRole::Blue, "B", "blue", false),
        ];
        let protection = BTreeMap::new();
        let id = |processing: StackColorProcessing| {
            let request = StackColorRequest {
                target_id: 7,
                kind: StackColorKind::Rgb,
                palette: None,
                force: false,
                crop: StackColorCrop::None,
                processing: Some(processing),
                input_sources: BTreeMap::new(),
            };
            color_job_id(&ColorJobIdentity {
                database_id: "db",
                project_id: 3,
                request: &request,
                label: "RGB",
                rc_astro_input_id: None,
                background_protection: &protection,
                color_calibration_basis: None,
                sources: &sources,
            })
            .unwrap()
        };
        let with = |input_processing, fwhm| StackColorProcessing {
            input_processing,
            input_deconvolutions: BTreeMap::from([(StackColorRole::Red, deconvolution(fwhm))]),
            ..defaults()
        };
        // A finished job is reused as it stands, and it records whether it
        // follows the cards, so the two choices are two jobs.
        assert_ne!(
            id(with(ColorInputProcessing::Own, 2.5)),
            id(with(ColorInputProcessing::Channels, 2.5))
        );
        // An own build written before the choice existed keeps its id: the
        // field is left out of `own` records.
        assert_eq!(
            id(with(ColorInputProcessing::Own, 2.5)),
            id(StackColorProcessing {
                input_deconvolutions: BTreeMap::from([(StackColorRole::Red, deconvolution(2.5))]),
                ..defaults()
            })
        );
        // A card applying other settings makes another preview.
        assert_ne!(
            id(with(ColorInputProcessing::Channels, 2.5)),
            id(with(ColorInputProcessing::Channels, 3.0))
        );
    }

    #[test]
    fn a_refreshed_color_preview_still_follows_the_cards() {
        use StackColorRole::{Blue, Green, Red};
        let mut previous = running_color_job("old-rgb", StackJobState::Completed);
        previous.kind = StackColorKind::Rgb;
        previous.palette = None;
        previous.target_id = 7;
        previous.sources = vec![
            channel(Red, "R", "old-r", false),
            channel(Green, "G", "g", false),
            channel(Blue, "B", "b", false),
        ];
        previous.processing = Some(StackColorProcessing {
            input_processing: ColorInputProcessing::Channels,
            ..defaults()
        });
        let target = sources(vec![
            channel(Red, "R", "new-r", false),
            channel(Green, "G", "g", false),
            channel(Blue, "B", "b", false),
        ]);
        let requests = plan_after_build(7, &target, &[previous], Some(&defaults()));
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].processing.as_ref().unwrap().input_processing,
            ColorInputProcessing::Channels,
            "the refresh reads the cards again when it is prepared"
        );
    }

    #[test]
    fn a_target_without_color_gets_its_richest_first_composite() {
        use StackColorRole::{Blue, Green, Ha, Luminance, Oiii, Red};
        let rgb = || {
            vec![
                channel(Red, "R", "r", false),
                channel(Red, "R", "wbpp-r", true),
                channel(Green, "G", "g", false),
                channel(Blue, "B", "b", false),
            ]
        };
        // R, G and B from PSF Guard's own stacks, beside a WBPP R.
        let requests = plan_after_build(7, &sources(rgb()), &[], Some(&defaults()));
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].kind, StackColorKind::Rgb);
        assert_eq!(requests[0].input_sources[&Red].job_id, "r");
        let stretches = &requests[0].processing.as_ref().unwrap().input_stretches;
        assert_eq!(
            stretches.keys().copied().collect::<Vec<_>>(),
            [Red, Green, Blue]
        );

        // With L too, LRGB comes first.
        let mut lrgb = rgb();
        lrgb.push(channel(Luminance, "L", "l", false));
        let requests = plan_after_build(7, &sources(lrgb), &[], Some(&defaults()));
        assert_eq!(requests[0].kind, StackColorKind::Lrgb);

        // Ha and OIII alone make a narrowband preview.
        let requests = plan_after_build(
            7,
            &sources(vec![
                channel(Ha, "Ha", "ha", false),
                channel(Oiii, "OIII", "o", false),
            ]),
            &[],
            Some(&defaults()),
        );
        assert_eq!(requests[0].kind, StackColorKind::Narrowband);
        assert!(requests[0].palette.is_some());

        // Without the person's defaults, as for an automatic refresh, no
        // first composite; and a role with two of PSF Guard's own stacks is
        // a choice left to the person.
        assert!(plan_after_build(7, &sources(rgb()), &[], None).is_empty());
        let mut two_reds = rgb();
        two_reds.push(channel(Red, "R", "r-300s", false));
        assert!(plan_after_build(7, &sources(two_reds), &[], Some(&defaults())).is_empty());
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
        // In hundredths of a unit, so a tool's own progress can move it.
        assert_eq!(entry.processed_units, 400);
        assert_eq!(entry.total_units, color_progress(3, None).total_units * 100);
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
    fn wbpp_masters_of_one_filter_are_told_apart_by_their_sub_exposure() {
        let cache = tempfile::tempdir().unwrap();
        let mut groups = vec![
            source_group("R", 0),
            source_group("R", 1),
            source_group("G", 2),
        ];
        for (group, seconds) in groups.iter_mut().zip([Some(60.0), Some(300.0), None]) {
            group.wbpp = Some(super::super::wbpp_stacks::WbppStackSource {
                master_file: "masterLight.xisf".into(),
                output_dir: "/wbpp".into(),
                exposure_seconds: seconds,
                drizzle: false,
                autocrop: true,
                imported_unix_seconds: 1,
            });
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
        let targets = collect_sources(cache.path(), &latest);
        let target = targets.get(&7).unwrap();
        let mut reds = target.by_role[&StackColorRole::Red]
            .iter()
            .map(|source| source.label.as_str())
            .collect::<Vec<_>>();
        reds.sort();
        assert_eq!(reds, ["R (WBPP, 300 s)", "R (WBPP, 60 s)"]);
        assert_eq!(target.by_role[&StackColorRole::Green][0].label, "G (WBPP)");
    }

    #[test]
    fn exposure_candidates_require_an_explicit_current_source() {
        let cache = tempfile::tempdir().unwrap();
        let mut groups = vec![
            source_group("R", 0),
            source_group("R", 1),
            source_group("G", 2),
            source_group("B", 3),
        ];
        for (index, group) in groups.iter_mut().enumerate() {
            let seconds = if index == 1 { 300.0 } else { 30.0 };
            group.group.exposure_group = Some(crate::server::exposure_groups::ExposureGroup {
                key: format!("{seconds}"),
                label: format!("{seconds} s"),
                min_seconds: Some(seconds),
                max_seconds: Some(seconds),
            });
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
        let targets = collect_sources(cache.path(), &latest);
        let target = targets.get(&7).unwrap();
        let catalog = availability(&targets);
        assert!(catalog[0].rgb_available);
        assert_eq!(catalog[0].source_candidates.len(), 4);
        assert_eq!(catalog[0].ambiguous_roles, [StackColorRole::Red]);
        assert_eq!(catalog[0].source_candidates[0].label, "R (30 s)");
        let mut request: StackColorRequest = serde_json::from_value(serde_json::json!({
            "target_id": 7, "kind": "rgb"
        }))
        .unwrap();
        assert!(select_sources(target, &request).is_err());
        let selected = &target.by_role[&StackColorRole::Red][0];
        request.input_sources.insert(
            StackColorRole::Red,
            StackColorSourceRef {
                job_id: selected.job_id.clone(),
                group_index: selected.group_index,
                artifact_revision: selected.artifact_revision.clone(),
            },
        );
        let sources = select_sources(target, &request).unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(sources[0].job_id, selected.job_id);
        let mut queued = running_color_job(&"a".repeat(64), StackJobState::Queued);
        queued.database_id = latest.database_id.clone();
        queued.project_id = latest.project_id;
        queued.target_id = request.target_id;
        queued.sources = sources.clone();
        assert!(validate_current_color_sources(&queued, &latest).is_ok());
        let mut rebuilt = latest.clone();
        rebuilt.groups[0].artifact_revision = "rebuilt-while-queued".into();
        assert!(validate_current_color_sources(&queued, &rebuilt).is_err());
        let mut regrouped = latest.clone();
        regrouped.groups.clear();
        assert!(validate_current_color_sources(&queued, &regrouped).is_err());
        let mut other_target = latest.clone();
        other_target.groups[0].group.target_id += 1;
        assert!(validate_current_color_sources(&queued, &other_target).is_err());
        let mut other_project = latest.clone();
        other_project.project_id += 1;
        assert!(validate_current_color_sources(&queued, &other_project).is_err());
        let mut stale = request.clone();
        stale
            .input_sources
            .get_mut(&StackColorRole::Red)
            .unwrap()
            .artifact_revision = "old".into();
        assert!(select_sources(target, &stale).is_err());
        let mut wrong_role = request.clone();
        let green = &target.by_role[&StackColorRole::Green][0];
        wrong_role.input_sources.insert(
            StackColorRole::Red,
            StackColorSourceRef {
                job_id: green.job_id.clone(),
                group_index: green.group_index,
                artifact_revision: green.artifact_revision.clone(),
            },
        );
        assert!(select_sources(target, &wrong_role).is_err());
        let mut foreign_target = request.clone();
        foreign_target
            .input_sources
            .get_mut(&StackColorRole::Red)
            .unwrap()
            .job_id = "f".repeat(64);
        assert!(select_sources(target, &foreign_target).is_err());
        request.input_sources.insert(
            StackColorRole::Ha,
            StackColorSourceRef {
                job_id: selected.job_id.clone(),
                group_index: selected.group_index,
                artifact_revision: selected.artifact_revision.clone(),
            },
        );
        assert!(validate_request(&request).is_err());

        let mut variant = sources.clone();
        variant[0] = target.by_role[&StackColorRole::Red][1].clone();
        assert_ne!(source_family_key(&sources), source_family_key(&variant));
        let original_key = source_family_key(&variant);
        variant.reverse();
        for source in &mut variant {
            source.job_id = "new".into();
            source.artifact_revision = "new".into();
            source.group_index += 3;
        }
        assert_eq!(original_key, source_family_key(&variant));
    }

    #[test]
    fn a_composition_that_waited_takes_the_newest_stack_of_each_family() {
        let cache = tempfile::tempdir().unwrap();
        let exposed = |filter: &str, index: usize, seconds: f64| {
            let mut group = source_group(filter, index);
            group.group.exposure_group = Some(crate::server::exposure_groups::ExposureGroup {
                key: format!("{seconds}"),
                label: format!("{seconds} s"),
                min_seconds: Some(seconds),
                max_seconds: Some(seconds),
            });
            let path = super::super::fits_path(cache.path(), &group.job_id, group.group.index);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, b"fixture").unwrap();
            group
        };
        let latest = LatestStackPreviews {
            schema_version: 1,
            database_id: "db".into(),
            project_id: 1,
            updated_unix_seconds: 1,
            groups: vec![
                exposed("R", 0, 30.0),
                exposed("R", 1, 300.0),
                exposed("G", 2, 300.0),
                exposed("B", 3, 300.0),
            ],
        };
        // A person picked the 300 s stacks while their channels were queued
        // to rebuild.
        let mut request: StackColorRequest = serde_json::from_value(serde_json::json!({
            "target_id": 7, "kind": "rgb"
        }))
        .unwrap();
        for (role, index) in [
            (StackColorRole::Red, 1),
            (StackColorRole::Green, 2),
            (StackColorRole::Blue, 3),
        ] {
            let group = &latest.groups[index];
            request.input_sources.insert(
                role,
                StackColorSourceRef {
                    job_id: group.job_id.clone(),
                    group_index: group.group.index,
                    artifact_revision: group.artifact_revision.clone(),
                },
            );
        }
        let families = families_in(cache.path(), &latest, &request);
        assert_eq!(families.len(), 3);

        // The 300 s red stack was rebuilt, and green's build left it with no
        // stack at all.
        let mut rebuilt = latest.clone();
        rebuilt.groups[1] = exposed("R", 9, 300.0);
        rebuilt.groups.remove(2);
        let moved = newest_sources_in(cache.path(), &rebuilt, &request, &families);
        let red = &moved.input_sources[&StackColorRole::Red];
        assert_eq!(
            (red.job_id.as_str(), red.artifact_revision.as_str()),
            (rebuilt.groups[1].job_id.as_str(), "rev-9"),
            "the newest stack of the same filter and exposure, not the 30 s one"
        );
        assert_eq!(
            moved.input_sources[&StackColorRole::Blue].job_id,
            latest.groups[3].job_id,
            "a stack still current stays"
        );
        assert_eq!(
            moved.input_sources[&StackColorRole::Green].job_id,
            latest.groups[2].job_id,
            "a family with no stack keeps what it named, and preparing says so"
        );
        let targets = collect_sources(cache.path(), &rebuilt);
        assert!(select_sources(&targets[&7], &moved).is_err());
    }

    #[test]
    fn single_sources_remain_automatic_and_non_mono_artifacts_are_excluded() {
        let cache = tempfile::tempdir().unwrap();
        let mut groups = vec![
            source_group("R", 0),
            source_group("G", 1),
            source_group("B", 2),
            source_group("R", 3),
        ];
        groups[3].group.output_channels = 3;
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
        let targets = collect_sources(cache.path(), &latest);
        let request: StackColorRequest = serde_json::from_value(serde_json::json!({
            "target_id": 7, "kind": "rgb"
        }))
        .unwrap();
        let sources = select_sources(&targets[&7], &request).unwrap();
        assert_eq!(sources.len(), 3);
        assert_eq!(source_family_key(&sources), "");

        // The same filters from WBPP are another family, so a WBPP composite
        // and PSF Guard's own never replace each other.
        let mut wbpp = sources.clone();
        for source in &mut wbpp {
            source.wbpp = true;
        }
        let wbpp_key = source_family_key(&wbpp);
        assert_ne!(wbpp_key, "");
        wbpp[0].wbpp = false;
        assert_ne!(source_family_key(&wbpp), wbpp_key);
    }

    #[test]
    fn latest_color_index_keeps_exposure_variants_and_replaces_only_the_same_family() {
        let cache = tempfile::tempdir().unwrap();
        let mut short = running_color_job(&"a".repeat(64), StackJobState::Completed);
        short.source_family_key = "short-family".into();
        let mut long = short.clone();
        long.job_id = "b".repeat(64);
        long.source_family_key = "long-family".into();
        persist_latest_color(cache.path(), &short).unwrap();
        persist_latest_color(cache.path(), &long).unwrap();
        short.job_id = "c".repeat(64);
        persist_latest_color(cache.path(), &short).unwrap();
        let latest: LatestStackColorPreviews = serde_json::from_slice(
            &std::fs::read(latest_color_path(cache.path(), short.project_id)).unwrap(),
        )
        .unwrap();
        assert_eq!(latest.jobs.len(), 2);
        assert_eq!(latest.jobs[0].job_id, short.job_id);
        assert_eq!(latest.jobs[1].job_id, long.job_id);
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
            input_sources: BTreeMap::new(),
        })
        .is_err());
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Lrgb,
            palette: Some(StackNarrowbandPalette::Sho),
            force: false,
            crop: StackColorCrop::None,
            processing: None,
            input_sources: BTreeMap::new(),
        })
        .is_err());
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Narrowband,
            palette: None,
            force: false,
            crop: StackColorCrop::None,
            processing: None,
            input_sources: BTreeMap::new(),
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
            input_sources: BTreeMap::new(),
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
    fn color_channels_share_one_background_model() {
        let (width, height) = (96, 64);
        // A deterministic ripple stands in for sky noise.
        let ripple = |x: usize, y: usize| ((x * 7 + y * 13) % 11) as f32 * 2.0e-4;
        let curved = LinearImage::new(
            width,
            height,
            1,
            (0..width * height)
                .map(|index| {
                    let (x, y) = (index % width, index / width);
                    let nx = x as f32 / (width - 1) as f32 * 2.0 - 1.0;
                    let ny = y as f32 / (height - 1) as f32 * 2.0 - 1.0;
                    0.2 - 0.05 * (nx * nx + ny * ny) + ripple(x, y)
                })
                .collect(),
        )
        .unwrap();
        let flat = LinearImage::new(
            width,
            height,
            1,
            (0..width * height)
                .map(|index| 0.2 + ripple(index % width, index / width))
                .collect(),
        )
        .unwrap();
        let extraction = StackBackgroundExtraction {
            config: BackgroundConfig {
                model: ModelConfig::Automatic {
                    max_degree: 2,
                    ridge: 1.0e-8,
                    rbf_smoothing: 0.01,
                    max_control_points: 192,
                    allow_radial_basis: false,
                    minimum_improvement: 0.08,
                },
                samples_per_axis: 12,
                sample_radius: Some(1),
                search_steps: 0,
                ..BackgroundConfig::default()
            },
            correction_mode: CorrectionMode::Subtract,
            strength: 1.0,
            protect_catalog_emission: false,
        };
        let no_fallback = |_: &str| panic!("unprotected fits need no fallback");
        let (curved_fit, _) =
            fit_channel_background(&curved, &extraction, Vec::new(), no_fallback).unwrap();
        let (flat_fit, _) =
            fit_channel_background(&flat, &extraction, Vec::new(), no_fallback).unwrap();
        assert!(matches!(
            curved_fit.model,
            FittedModel::Polynomial { degree: 2, .. }
        ));
        assert!(matches!(
            flat_fit.model,
            FittedModel::Polynomial { degree: 0, .. }
        ));

        let shared = seiza_background::select_shared_model(
            &extraction.config.model,
            &[&curved_fit, &flat_fit],
        )
        .unwrap();
        assert!(fit_uses_model(&curved_fit, &shared));
        assert!(!fit_uses_model(&flat_fit, &shared));

        let refit =
            refit_channel_background(&flat, &extraction, Vec::new(), &flat_fit, &shared).unwrap();
        assert!(fit_uses_model(&refit, &shared));
        let selection = refit.diagnostics.model_selection.as_ref().unwrap();
        assert_eq!(selection.selected, "polynomial_2");
        assert_eq!(
            selection.candidates,
            flat_fit
                .diagnostics
                .model_selection
                .as_ref()
                .unwrap()
                .candidates
        );
    }

    #[test]
    fn progress_ledger_accounts_for_every_pipeline_phase() {
        let processing = StackColorProcessing {
            input_processing: ColorInputProcessing::Own,
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
            input_rc_astro: BTreeMap::new(),
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
            color_calibration: Some(StackColorCalibration::default()),
        };

        let progress = color_progress(3, Some(&processing));

        assert_eq!(progress.phases.len(), 14);
        assert_eq!(progress.total_units, 26);
        assert!(progress.phases.iter().any(|phase| {
            phase.phase == StackColorProgressPhase::CalibratingColor && phase.total_units == 1
        }));
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
    fn rgb_and_lrgb_calibrate_by_default_and_narrowband_never() {
        let mut processing = defaults();
        resolve_color_calibration(StackColorKind::Rgb, &mut processing);
        assert_eq!(
            processing.color_calibration,
            Some(StackColorCalibration::default())
        );

        let off = StackColorCalibration {
            enabled: false,
            ..StackColorCalibration::default()
        };
        processing.color_calibration = Some(off);
        resolve_color_calibration(StackColorKind::Lrgb, &mut processing);
        assert_eq!(processing.color_calibration, Some(off));

        processing.color_calibration = Some(StackColorCalibration::default());
        resolve_color_calibration(StackColorKind::Narrowband, &mut processing);
        assert_eq!(processing.color_calibration, None);
        assert!(serde_json::to_value(&processing)
            .unwrap()
            .get("color_calibration")
            .is_none());
    }

    fn calibration_basis(size_bytes: u64) -> StackColorCalibrationBasis {
        StackColorCalibrationBasis {
            reference_image_id: Some(41),
            solution_fingerprint: Some("solve".into()),
            catalog: Some(crate::astrometry::AstrometryCatalogFileSignature {
                name: "gaia_photometry".into(),
                path: "/catalogs/stars-gaia-photometry.bin".into(),
                format: "SEIZAPH1".into(),
                size_bytes,
                modified_unix_seconds: 1,
                modified_subsec_nanos: 0,
                sha256: None,
            }),
        }
    }

    #[test]
    fn the_cache_key_follows_color_calibration_settings_and_basis() {
        let sources = vec![
            channel(StackColorRole::Red, "R", "red", false),
            channel(StackColorRole::Green, "G", "green", false),
            channel(StackColorRole::Blue, "B", "blue", false),
        ];
        let mut processing = defaults();
        resolve_color_calibration(StackColorKind::Rgb, &mut processing);
        let request = |processing: StackColorProcessing| StackColorRequest {
            target_id: 7,
            kind: StackColorKind::Rgb,
            palette: None,
            force: false,
            crop: StackColorCrop::None,
            processing: Some(processing),
            input_sources: BTreeMap::new(),
        };
        let protection = BTreeMap::new();
        let id = |request: &StackColorRequest, basis: Option<&StackColorCalibrationBasis>| {
            color_job_id(&ColorJobIdentity {
                database_id: "db",
                project_id: 3,
                request,
                label: "RGB",
                rc_astro_input_id: None,
                background_protection: &protection,
                color_calibration_basis: basis,
                sources: &sources,
            })
            .unwrap()
        };
        let basis = calibration_basis(10);
        let calibrated = request(processing.clone());
        let baseline = id(&calibrated, Some(&basis));
        assert_eq!(baseline, id(&calibrated, Some(&calibration_basis(10))));

        // A reinstalled catalog, or none at all, is a different preview.
        assert_ne!(baseline, id(&calibrated, Some(&calibration_basis(11))));
        let unsolved = StackColorCalibrationBasis {
            solution_fingerprint: None,
            ..basis.clone()
        };
        assert_ne!(baseline, id(&calibrated, Some(&unsolved)));

        let mut neutral_sky_off = processing.clone();
        neutral_sky_off.color_calibration = Some(StackColorCalibration {
            neutralize_background: false,
            ..StackColorCalibration::default()
        });
        assert_ne!(baseline, id(&request(neutral_sky_off), Some(&basis)));

        let mut off = processing;
        off.color_calibration = Some(StackColorCalibration {
            enabled: false,
            ..StackColorCalibration::default()
        });
        assert_ne!(baseline, id(&request(off), None));
    }

    fn mono(width: usize, height: usize, value: f32) -> LinearImage {
        LinearImage::new(width, height, 1, vec![value; width * height]).unwrap()
    }

    #[test]
    fn a_calibration_that_cannot_run_leaves_the_channels_alone() {
        use super::super::color_calibration::{plan, StackColorCalibrationSkipReason};

        let solution = super::super::color_calibration::tests::solution();
        let layout = stack_orientation(64, 48, seiza_stacking::AffineTransform::IDENTITY);
        let catalog = calibration_basis(10).catalog;
        let channels = || {
            BTreeMap::from([
                (StackColorRole::Red, mono(64, 48, 0.3)),
                (StackColorRole::Green, mono(64, 48, 0.2)),
                (StackColorRole::Blue, mono(64, 48, 0.1)),
            ])
        };
        let gone =
            || -> Result<Arc<seiza::catalog::PhotometryCatalog>, String> { Err("gone".into()) };
        let reason = |outcome: StackColorCalibrationOutcome| match outcome {
            StackColorCalibrationOutcome::Skipped(skip) => skip.reason,
            calibrated => panic!("{calibrated:?}"),
        };
        let settings = StackColorCalibration::default();

        // No solve or no catalog is known when the job is prepared.
        let no_solve = plan(settings, Some(41), Some(&layout), None, catalog.clone());
        assert_eq!(
            no_solve.placement.unwrap_err().reason,
            StackColorCalibrationSkipReason::NoPlateSolve
        );

        // A catalog that has gone by the time the job runs.
        let ready = plan(
            settings,
            Some(41),
            Some(&layout),
            Some(&solution),
            catalog.clone(),
        );
        let placement = ready.placement.as_ref().unwrap();
        let mut images = channels();
        assert_eq!(
            reason(calibrate_registered_channels(
                placement,
                &settings,
                &mut images,
                &gone
            )),
            StackColorCalibrationSkipReason::NoGaiaCatalog
        );
        assert_eq!(images, channels());

        // A layout recorded for another size of stack places nothing.
        let mismatched = stack_orientation(640, 480, seiza_stacking::AffineTransform::IDENTITY);
        let stale = plan(
            settings,
            Some(41),
            Some(&mismatched),
            Some(&solution),
            catalog,
        );
        let unopened = || -> Result<Arc<seiza::catalog::PhotometryCatalog>, String> {
            panic!("a stale layout opens no catalog")
        };
        assert_eq!(
            reason(calibrate_registered_channels(
                stale.placement.as_ref().unwrap(),
                &settings,
                &mut images,
                &unopened
            )),
            StackColorCalibrationSkipReason::Failed
        );
        assert_eq!(images, channels());

        // A composition that cannot be calibrated is still stretched, each
        // channel on its own as it would have been without calibration.
        let auto = super::super::stretch::StackStretchRequest {
            model: seiza_stretch::StretchModel::AutoMtf(seiza_stretch::StretchParams::default()),
            color_strategy: seiza_stretch::ColorStrategy::Linked,
        };
        let composition = color_calibration::interleave(
            &images[&StackColorRole::Red],
            &images[&StackColorRole::Green],
            &images[&StackColorRole::Blue],
        )
        .unwrap();
        let stretched = calibrate_and_stretch_composition(
            placement,
            &settings,
            composition.clone(),
            ReferenceRegion {
                x: 0,
                y: 0,
                width: 64,
                height: 48,
            },
            std::slice::from_ref(&auto),
            &[],
            &gone,
        )
        .unwrap();
        assert_eq!(
            reason(stretched.calibration.unwrap()),
            StackColorCalibrationSkipReason::NoGaiaCatalog
        );
        let unlinked = composition_stretch(std::slice::from_ref(&auto), &[], false)
            .unwrap()
            .unwrap();
        assert_eq!(
            unlinked.stages()[0].color_strategy,
            seiza_stretch::ColorStrategy::Unlinked
        );
        let (expected, _) = stretch_linear_composition(&composition, &unlinked).unwrap();
        assert_eq!(stretched.image, expected);
        assert_eq!(stretched.plans.len(), 1);
    }

    #[test]
    fn shared_channel_stretches_wait_for_the_composition_and_custom_ones_do_not() {
        let auto = || super::super::stretch::StackStretchRequest {
            model: seiza_stretch::StretchModel::AutoMtf(seiza_stretch::StretchParams::default()),
            color_strategy: seiza_stretch::ColorStrategy::Linked,
        };
        let mut processing = defaults();
        for role in RGB_ROLES {
            processing.input_stretches.insert(role, vec![auto()]);
        }
        let rgb = composed_roles(StackColorKind::Rgb, None);
        assert_eq!(
            shared_input_stages(&processing, &rgb).map(|stages| stages.len()),
            Some(1)
        );
        // LRGB shares only when luminance has the same stretch too.
        let lrgb = composed_roles(StackColorKind::Lrgb, None);
        assert!(shared_input_stages(&processing, &lrgb).is_none());
        processing
            .input_stretches
            .insert(StackColorRole::Luminance, vec![auto()]);
        assert!(shared_input_stages(&processing, &lrgb).is_some());

        processing.input_stretches.insert(
            StackColorRole::Blue,
            vec![super::super::stretch::StackStretchRequest {
                model: seiza_stretch::StretchModel::Identity,
                color_strategy: seiza_stretch::ColorStrategy::Linked,
            }],
        );
        assert!(shared_input_stages(&processing, &rgb).is_none());

        // No stretch on any channel is shared too: the output stages, if
        // any, stretch the calibrated composition.
        for role in RGB_ROLES {
            processing.input_stretches.insert(role, Vec::new());
        }
        assert_eq!(
            shared_input_stages(&processing, &rgb).map(|stages| stages.len()),
            Some(0)
        );
        assert!(composition_stretch(&[], &[], true).unwrap().is_none());
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
            label: role.label().into(),
            exposure_group: None,
            job_id: format!("{index:064x}"),
            group_index: 0,
            artifact_revision: format!("revision-{index}"),
            accepted_frames: 4,
            wbpp: false,
            total_exposure_seconds: Some(1200.0),
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
            resolved_rc_astro: BTreeMap::new(),
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
        let green = apply_input_stretch_stack(
            &stretch,
            &covered(2, 0).data,
            ImageShape::new(8, 8, 1),
            |_| {},
        )
        .unwrap();
        let blue = apply_input_stretch_stack(
            &stretch,
            &covered(0, 3).data,
            ImageShape::new(8, 8, 1),
            |_| {},
        )
        .unwrap();
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
