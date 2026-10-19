//! Reversible deconvolution and display-stretch variants rendered from cached
//! linear stack FITS.

use super::{
    save_png_atomic, validate_job_id, StackPreviewImageQuery, StackPreviewImageSize,
    PREVIEW_MAX_DIMENSION,
};
use axum::{
    body::Body,
    extract::{Path, Query},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::Response,
    Json,
};
use seiza_deconvolution::{
    deconvolve_masked, ChannelDiagnostics, DeconvolutionConfig, ALGORITHM_VERSION,
};
use seiza_fits::{HeaderValue, WriteHeaderCard};
use seiza_stacking::{write_processed_image_fits_f32, LinearImage};
use seiza_stretch::{
    ColorStrategy, ImageShape, RobustStatistics, StretchAnalysis, StretchConfig, StretchModel,
    StretchParams, StretchPlan,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
use tokio_util::io::ReaderStream;

use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;

const STRETCH_CACHE_VERSION: u32 = 2;
const DECONVOLUTION_CACHE_VERSION: u32 = 1;
const STRETCH_ANALYSIS_SAMPLES: usize = 200_000;
const STRETCH_BYTES_PER_SAMPLE: u64 = 16;
const DECONVOLUTION_BYTES_PER_SAMPLE: u64 = 40;
const LINEAR_BLACK_PERCENTILE: f64 = 0.001;
const LINEAR_WHITE_PERCENTILE: f64 = 0.999;
/// The Seiza release whose stretch output this build reproduces. Stack
/// previews and stretch runs key on it rather than on the crate version.
pub(super) const SEIZA_STRETCH_OUTPUT_VERSION: &str = seiza_stretch::OUTPUT_VERSION;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StackDeconvolutionResult {
    pub config: DeconvolutionConfig,
    pub channels: Vec<ChannelDiagnostics>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StackStretchRequest {
    pub model: StretchModel,
    #[serde(default)]
    pub color_strategy: ColorStrategy,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StackViewProcessingRequest {
    #[serde(flatten)]
    pub stretch: StackStretchRequest,
    /// Optional linear-light restoration. Omitted by default so existing and
    /// newly created previews retain their original pixels unless requested.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deconvolution: Option<DeconvolutionConfig>,
    /// Optional RC-Astro tool chain (BXT, NXT, SXT) applied to the linear
    /// data after deconvolution and before the stretch. Star removal stores
    /// the stars image beside the starless result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rc_astro: Option<super::rc_astro::RcAstroProcessing>,
    /// Photometric color calibration of a one-shot-color stack against Gaia,
    /// before the stretch. A color stack without it is calibrated with the
    /// defaults; a mono stack never is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration: Option<super::color_calibration::StackColorCalibration>,
}

impl StackStretchRequest {
    pub(super) fn config(&self) -> StretchConfig {
        StretchConfig {
            model: self.model.clone(),
            color_strategy: self.color_strategy,
            max_analysis_samples: STRETCH_ANALYSIS_SAMPLES,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StackStretchSourceTransfer {
    Linear,
    DisplayReferred,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StackStretchPreview {
    pub schema_version: u32,
    pub stretch_id: String,
    pub stretch_version: String,
    #[serde(default)]
    pub deconvolution_version: Option<String>,
    #[serde(default)]
    pub deconvolution_id: Option<String>,
    pub config: StretchConfig,
    #[serde(default)]
    pub request: Option<StackViewProcessingRequest>,
    pub resolved_plan: serde_json::Value,
    pub source_transfer: StackStretchSourceTransfer,
    pub input_range: Option<StackStretchInputRange>,
    pub linked_statistics: RobustStatistics,
    pub channel_statistics: Vec<Option<RobustStatistics>>,
    pub luminance_statistics: Option<RobustStatistics>,
    #[serde(default)]
    pub deconvolution: Option<StackDeconvolutionResult>,
    #[serde(default)]
    pub rc_astro: Option<super::rc_astro::StackRcAstroResult>,
    #[serde(default)]
    pub rc_astro_id: Option<String>,
    /// What color calibration did for a one-shot-color stack.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration: Option<super::color_calibration::StackColorCalibrationOutcome>,
    /// The solve and Gaia catalog that calibration rested on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration_basis: Option<super::color_calibration::StackColorCalibrationBasis>,
    /// Why this processing is out of date, worked out when it is served: a
    /// new solve or Gaia catalog since. Never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_calibration_outdated: Option<String>,
    pub preview_url: String,
    pub original_preview_url: String,
    #[serde(default)]
    pub fits_url: Option<String>,
    /// The stretched stars image, present when star removal kept one.
    #[serde(default)]
    pub stars_preview_url: Option<String>,
    #[serde(default)]
    pub stars_original_preview_url: Option<String>,
    #[serde(default)]
    pub stars_fits_url: Option<String>,
    /// Present when this result belongs to the stack's previous revision and
    /// is shown while the server runs the same processing on the new one.
    /// Never stored in a manifest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reprocessing: Option<Reprocessing>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct StackStretchInputRange {
    pub black: f32,
    pub white: f32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CachedDeconvolution {
    schema_version: u32,
    deconvolution_id: String,
    algorithm_version: u32,
    config: DeconvolutionConfig,
    channels: Vec<ChannelDiagnostics>,
}

pub(super) fn deconvolution_version() -> String {
    format!("algorithm-{ALGORITHM_VERSION}")
}

fn deconvolution_cache_id(
    database_id: &str,
    source_key: &str,
    source_revision: &str,
    config: &DeconvolutionConfig,
) -> Result<String, AppError> {
    let encoded = serde_json::to_vec(config).map_err(|error| {
        AppError::InternalError(format!("Failed to encode deconvolution request: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(source_key.as_bytes());
    hasher.update(source_revision.as_bytes());
    hasher.update(DECONVOLUTION_CACHE_VERSION.to_le_bytes());
    hasher.update(ALGORITHM_VERSION.to_le_bytes());
    hasher.update(encoded);
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(id)
}

pub(crate) fn default_linear_config() -> StretchConfig {
    StretchConfig::auto_mtf(
        StretchParams {
            target_median: 0.2,
            shadows_clip: -2.8,
        },
        STRETCH_ANALYSIS_SAMPLES,
    )
}

pub(super) fn display_identity_config() -> StretchConfig {
    StretchConfig {
        model: StretchModel::Identity,
        color_strategy: ColorStrategy::Linked,
        max_analysis_samples: STRETCH_ANALYSIS_SAMPLES,
    }
}

pub(crate) struct StretchRender {
    plan: StretchPlan,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StackPreviewRenderPhase {
    Original,
    Screen,
}

pub(super) fn render_image_previews_atomic(
    image: &LinearImage,
    config: &StretchConfig,
    source_transfer: StackStretchSourceTransfer,
    screen_destination: &FsPath,
    original_destination: &FsPath,
) -> Result<serde_json::Value, String> {
    render_image_previews_atomic_with_progress(
        image,
        config,
        source_transfer,
        screen_destination,
        original_destination,
        |_| {},
    )
}

pub(super) fn render_image_preview_atomic(
    image: &LinearImage,
    config: &StretchConfig,
    source_transfer: StackStretchSourceTransfer,
    destination: &FsPath,
) -> Result<serde_json::Value, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "Stack preview path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let normalized = if source_transfer == StackStretchSourceTransfer::Linear {
        Some(normalize_linear_image(image)?.0)
    } else {
        None
    };
    let prepared = normalized.as_ref().unwrap_or(image);
    let (dynamic, render) = render_dynamic_image(prepared, config)?;
    save_png_atomic(&dynamic, destination)?;
    serde_json::to_value(render.plan).map_err(|error| error.to_string())
}

pub(super) fn render_image_previews_atomic_with_progress(
    image: &LinearImage,
    config: &StretchConfig,
    source_transfer: StackStretchSourceTransfer,
    screen_destination: &FsPath,
    original_destination: &FsPath,
    progress: impl FnMut(StackPreviewRenderPhase),
) -> Result<serde_json::Value, String> {
    let normalized = if source_transfer == StackStretchSourceTransfer::Linear {
        Some(normalize_linear_image(image)?.0)
    } else {
        None
    };
    let prepared = normalized.as_ref().unwrap_or(image);
    render_image_previews_with_details(
        prepared,
        config,
        screen_destination,
        original_destination,
        progress,
    )
    .and_then(|render| serde_json::to_value(render.plan).map_err(|error| error.to_string()))
}

pub(crate) fn normalize_linear_image(
    image: &LinearImage,
) -> Result<(LinearImage, StackStretchInputRange), String> {
    let sampled_pixels = (STRETCH_ANALYSIS_SAMPLES / image.channels).max(1);
    let pixel_step = (image.pixel_count() / sampled_pixels).max(1);
    let mut sample = (0..image.pixel_count())
        .step_by(pixel_step)
        .flat_map(|pixel| {
            let start = pixel * image.channels;
            image.data[start..start + image.channels].iter().copied()
        })
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if sample.len() < 2 {
        return Err("Stack has too few finite samples to normalize".into());
    }
    sample.sort_unstable_by(f32::total_cmp);
    let percentile = |value: f64| {
        let index = ((sample.len() - 1) as f64 * value).round() as usize;
        sample[index]
    };
    let range = StackStretchInputRange {
        black: percentile(LINEAR_BLACK_PERCENTILE),
        white: percentile(LINEAR_WHITE_PERCENTILE),
    };
    let span = range.white - range.black;
    if !span.is_finite() || span <= f32::EPSILON {
        return Err("Stack has no usable robust display range".into());
    }
    let data = image
        .data
        .iter()
        .map(|value| {
            if value.is_finite() {
                (*value - range.black) / span
            } else {
                f32::NAN
            }
        })
        .collect();
    let normalized = LinearImage::new(image.width, image.height, image.channels, data)
        .map_err(|error| error.to_string())?;
    Ok((normalized, range))
}

fn render_image_previews_with_details(
    image: &LinearImage,
    config: &StretchConfig,
    screen_destination: &FsPath,
    original_destination: &FsPath,
    mut progress: impl FnMut(StackPreviewRenderPhase),
) -> Result<StretchRender, String> {
    for destination in [screen_destination, original_destination] {
        let parent = destination
            .parent()
            .ok_or_else(|| "Stack preview path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let (dynamic, render) = render_dynamic_image(image, config)?;
    progress(StackPreviewRenderPhase::Original);
    save_png_atomic(&dynamic, original_destination)?;
    progress(StackPreviewRenderPhase::Screen);
    let resized = dynamic.resize(
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_DIMENSION,
        image::imageops::FilterType::Lanczos3,
    );
    save_png_atomic(&resized, screen_destination)?;
    Ok(render)
}

/// Render one screen/original PNG pair by applying an already-resolved plan
/// — how the stars image reuses the starless image's transform.
fn render_previews_with_plan(
    image: &LinearImage,
    plan: &StretchPlan,
    screen_destination: &FsPath,
    original_destination: &FsPath,
) -> Result<(), String> {
    for destination in [screen_destination, original_destination] {
        let parent = destination
            .parent()
            .ok_or_else(|| "Stack preview path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let pixels = plan
        .apply_u8_image(&image.data, image_shape(image))
        .map_err(|error| error.to_string())?;
    let dynamic = if image.channels == 1 {
        image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(image.width as u32, image.height as u32, pixels)
                .ok_or_else(|| "Stack preview dimensions do not match pixels".to_string())?,
        )
    } else {
        image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(image.width as u32, image.height as u32, pixels)
                .ok_or_else(|| "Stack preview dimensions do not match pixels".to_string())?,
        )
    };
    save_png_atomic(&dynamic, original_destination)?;
    let resized = dynamic.resize(
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_DIMENSION,
        image::imageops::FilterType::Lanczos3,
    );
    save_png_atomic(&resized, screen_destination)
}

/// The shape Seiza needs to run a stretch over the whole image; the layered
/// stretch blurs it, so it cannot work sample by sample.
pub(crate) fn image_shape(image: &LinearImage) -> ImageShape {
    ImageShape::new(image.width, image.height, image.channels)
}

pub(crate) fn render_dynamic_image(
    image: &LinearImage,
    config: &StretchConfig,
) -> Result<(image::DynamicImage, StretchRender), String> {
    if !matches!(image.channels, 1 | 3) {
        return Err(format!(
            "Stack stretch requires one or three channels, found {}",
            image.channels
        ));
    }
    let analysis =
        StretchAnalysis::analyze(&image.data, image.channels, config.max_analysis_samples)
            .map_err(|error| error.to_string())?;
    let plan = config
        .resolve(&analysis)
        .map_err(|error| error.to_string())?;
    let pixels = plan
        .apply_u8_image(&image.data, image_shape(image))
        .map_err(|error| error.to_string())?;
    let dynamic = if image.channels == 1 {
        image::DynamicImage::ImageLuma8(
            image::GrayImage::from_raw(image.width as u32, image.height as u32, pixels)
                .ok_or_else(|| "Stack preview dimensions do not match pixels".to_string())?,
        )
    } else {
        image::DynamicImage::ImageRgb8(
            image::RgbImage::from_raw(image.width as u32, image.height as u32, pixels)
                .ok_or_else(|| "Stack preview dimensions do not match pixels".to_string())?,
        )
    };
    Ok((dynamic, StretchRender { plan }))
}

/// What an apply request produced: the finished preview, or a note that
/// the same request is being computed and the client should poll.
pub(super) enum StretchApplyOutcome {
    Ready(Box<StackStretchPreview>),
    /// The work runs detached (RC-Astro chains take minutes — far past any
    /// reverse-proxy timeout); the client re-sends the same request until
    /// it turns Ready, each poll carrying the run's live progress. Keeps
    /// generation out of the HTTP request path.
    Pending {
        stretch_id: String,
    },
}

/// One entry per stretch id being computed right now, so identical
/// concurrent requests wait on the same run instead of starting duplicates.
static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(Default::default);
/// The failure of a detached run, held for the next poll of that id.
static FAILURES: std::sync::LazyLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::LazyLock::new(Default::default);
/// Live progress of a running computation, served to the pending poll and
/// the activity list: the step under way and, while an RC-Astro tool runs,
/// the chain's overall fraction. A run still waiting for the worker has no
/// entry here; the manager's line says what it waits for.
static PROGRESS: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, PendingProgress>>,
> = std::sync::LazyLock::new(Default::default);

#[derive(Debug, Clone, Serialize)]
pub(super) struct PendingProgress {
    pub stage: String,
    /// Known while an RC-Astro tool reports it; a step that cannot say,
    /// such as deconvolution, leaves the last fraction or none.
    pub fraction: Option<f32>,
}

/// An RC-Astro tool's progress: its name and the chain's overall fraction.
pub(super) fn report_progress(stretch_id: &str, stage: &str, fraction: f32) {
    if let Ok(mut progress) = PROGRESS.lock() {
        progress.insert(
            stretch_id.to_string(),
            PendingProgress {
                stage: stage.to_string(),
                fraction: Some(fraction),
            },
        );
    }
}

/// A step that cannot say how far it has got. The fraction so far stays, so
/// rendering after a finished chain still reads 100%.
fn report_stage(stretch_id: &str, stage: &str) {
    if let Ok(mut progress) = PROGRESS.lock() {
        let fraction = progress.get(stretch_id).and_then(|entry| entry.fraction);
        progress.insert(
            stretch_id.to_string(),
            PendingProgress {
                stage: stage.to_string(),
                fraction,
            },
        );
    }
}

/// The live progress of a running computation, if it has reported any.
pub(super) fn run_progress(stretch_id: &str) -> Option<PendingProgress> {
    PROGRESS.lock().ok()?.get(stretch_id).cloned()
}

#[cfg(test)]
pub(super) fn clear_progress(stretch_id: &str) {
    if let Ok(mut progress) = PROGRESS.lock() {
        progress.remove(stretch_id);
    }
}

/// Removes its id from the in-flight set (and its progress entry) on drop —
/// panic or success alike.
struct InFlightToken(String);

impl Drop for InFlightToken {
    fn drop(&mut self) {
        if let Ok(mut in_flight) = IN_FLIGHT.lock() {
            in_flight.remove(&self.0);
        }
        if let Ok(mut progress) = PROGRESS.lock() {
            progress.remove(&self.0);
        }
    }
}

fn try_begin_in_flight(stretch_id: &str) -> Option<InFlightToken> {
    let mut in_flight = IN_FLIGHT.lock().ok()?;
    if in_flight.insert(stretch_id.to_string()) {
        Some(InFlightToken(stretch_id.to_string()))
    } else {
        None
    }
}

struct StretchIdentity {
    stretch_id: String,
    deconvolution_id: Option<String>,
    /// Per-prefix cache ids for the RC-Astro chain, canonical order; the
    /// last one is the chain's public identity.
    rc_astro_chain: Option<Vec<String>>,
    selection_path: PathBuf,
    request: StackViewProcessingRequest,
}

impl StretchIdentity {
    fn rc_astro_id(&self) -> Option<&str> {
        self.rc_astro_chain
            .as_ref()
            .and_then(|chain| chain.last())
            .map(String::as_str)
    }
}

fn read_cached_manifest(stack_root: &FsPath, stretch_id: &str) -> Option<StackStretchPreview> {
    let bytes = std::fs::read(stretch_manifest_path(stack_root, stretch_id)).ok()?;
    let cached = serde_json::from_slice::<StackStretchPreview>(&bytes).ok()?;
    stretch_artifacts_exist(stack_root, stretch_id, &cached).then_some(cached)
}

#[derive(Default, Deserialize, Serialize)]
struct ProcessingSelection {
    selected: Option<String>,
    requested: String,
    desired: Option<String>,
    #[serde(default)]
    source: Option<ProcessingSource>,
    /// The previous revision's processing, carried to this one when its
    /// stack refreshed. Its result shows until the server has run the same
    /// request here; once a revision has carried, it never carries again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    carried: Option<CarriedProcessing>,
}

/// Where a carried re-apply stands, as the selection file keeps it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CarryState {
    /// Settling, queued, or running.
    Waiting,
    /// The re-apply failed; the previous result stays, with the reason.
    Failed,
    /// Turned off, taken out of the line, or replaced by a person's own
    /// processing. Nothing carried shows.
    Stopped,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct CarriedProcessing {
    /// The previous revision's result, shown meanwhile.
    from_stretch_id: String,
    from_revision: String,
    state: CarryState,
    /// Carried from a build a person started: the re-apply is theirs.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    personal: bool,
    /// The re-apply's run, once it is queued.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// How a carried result's re-apply stands, for the card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReprocessingState {
    /// Waiting for the stack's refreshes to settle and the worker to free.
    Waiting,
    Queued,
    Running,
    Failed,
}

/// Set on a processing result that belongs to the previous revision of the
/// stack, shown while the server runs the same processing on the new one.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Reprocessing {
    pub state: ReprocessingState,
    /// The revision the shown result was made from.
    pub from_revision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct ProcessingSource {
    key: String,
    revision: String,
}

static SELECTION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn selection_path(stack_root: &FsPath, source_key: &str, source_revision: &str) -> PathBuf {
    let mut hasher = Sha256::new();
    hasher.update(source_key.as_bytes());
    hasher.update([0]);
    hasher.update(source_revision.as_bytes());
    let mut id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    crate::server::storage::stack_processing(stack_root).join(format!("{id}.json"))
}

fn read_selection(path: &FsPath) -> Option<ProcessingSelection> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

#[cfg(test)]
fn request_selection(path: &FsPath, generation: &str) -> Result<(), String> {
    request_selection_with_source(path, generation, None, false)
}

/// A new request for this revision. A person's own replaces anything carried
/// from the previous revision; the server's re-apply keeps it.
fn request_selection_with_source(
    path: &FsPath,
    generation: &str,
    source: Option<ProcessingSource>,
    keep_carried: bool,
) -> Result<(), String> {
    let _guard = SELECTION_LOCK.lock().map_err(|error| error.to_string())?;
    let mut selection = read_selection(path).unwrap_or_default();
    selection.requested = generation.into();
    selection.desired = None;
    selection.source = source;
    if !keep_carried {
        selection.carried = None;
    }
    write_json_atomic(path, &selection)
}

fn bind_selection(path: &FsPath, generation: &str, stretch_id: &str) -> Result<(), String> {
    let _guard = SELECTION_LOCK.lock().map_err(|error| error.to_string())?;
    if let Some(mut selection) = read_selection(path)
        && selection.requested == generation
    {
        selection.desired = Some(stretch_id.into());
        write_json_atomic(path, &selection)?;
    }
    Ok(())
}

fn complete_selection(path: &FsPath, stretch_id: &str) -> Result<(), String> {
    let _guard = SELECTION_LOCK.lock().map_err(|error| error.to_string())?;
    if let Some(mut selection) = read_selection(path)
        && selection.desired.as_deref() == Some(stretch_id)
    {
        // The variant left behind keeps its week from now, not from when it
        // was made.
        if let Some(previous) = selection.selected.as_deref()
            && previous != stretch_id
            && validate_job_id(previous).is_ok()
            && let Some(stack_root) = path.parent().and_then(FsPath::parent)
        {
            touch_dir(&stretch_dir(stack_root, previous));
        }
        selection.selected = Some(stretch_id.into());
        write_json_atomic(path, &selection)?;
    }
    Ok(())
}

pub(super) fn selected_processing(
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
) -> Result<Option<StackStretchPreview>, AppError> {
    let _guard = SELECTION_LOCK
        .lock()
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let selected = read_selection(&selection_path(stack_root, source_key, source_revision))
        .and_then(|selection| selection.selected)
        .filter(|id| validate_job_id(id).is_ok());
    Ok(selected.and_then(|id| read_cached_manifest(stack_root, &id)))
}

pub(super) fn clear_selected_processing(
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
) -> Result<(), AppError> {
    let _guard = SELECTION_LOCK
        .lock()
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let path = selection_path(stack_root, source_key, source_revision);
    // A revision that carried keeps a stopped mark, so it never carries again.
    if let Some(selection) = read_selection(&path)
        && let Some(mut carried) = selection.carried
    {
        carried.state = CarryState::Stopped;
        return write_json_atomic(
            &path,
            &ProcessingSelection {
                selected: None,
                requested: super::new_artifact_revision(),
                desired: None,
                source: selection.source,
                carried: Some(carried),
            },
        )
        .map_err(AppError::InternalError);
    }
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(AppError::InternalError(format!(
            "Failed to clear processing selection: {error}"
        ))),
    }
}

/// Mark a cached result folder as just used. Best effort: a folder that
/// cannot be opened (Windows opens no folders this way) keeps its old time.
fn touch_dir(path: &FsPath) {
    if let Ok(folder) = std::fs::File::open(path) {
        let _ = folder.set_modified(std::time::SystemTime::now());
    }
}

/// Stretch and deconvolution results a processing selection still names:
/// the one selected, and the one it is waiting for. A stretch keeps the
/// deconvolution it was made from. Fails with the folder or selection it
/// could not read: without it, a selected result would look unreferenced.
pub(super) fn kept_processing(
    stack_root: &FsPath,
) -> Result<
    (
        std::collections::HashSet<String>,
        std::collections::HashSet<String>,
    ),
    PathBuf,
> {
    let folder = crate::server::storage::stack_processing(stack_root);
    let mut stretches = std::collections::HashSet::new();
    {
        let _guard = SELECTION_LOCK.lock().map_err(|_| folder.clone())?;
        let entries = match std::fs::read_dir(&folder) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return Err(folder),
        };
        for entry in entries.into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let selection = read_selection(&path).ok_or(path)?;
            // A result carried to a newer revision shows there until that
            // revision has its own.
            let carried = selection
                .carried
                .filter(|carried| carried.state != CarryState::Stopped)
                .map(|carried| carried.from_stretch_id);
            stretches.extend(
                [selection.selected, selection.desired, carried]
                    .into_iter()
                    .flatten()
                    .filter(|id| validate_job_id(id).is_ok()),
            );
        }
    }
    let deconvolutions = stretches
        .iter()
        .filter_map(|id| std::fs::read(stretch_manifest_path(stack_root, id)).ok())
        .filter_map(|bytes| serde_json::from_slice::<StackStretchPreview>(&bytes).ok())
        .filter_map(|preview| preview.deconvolution_id)
        .collect();
    Ok((stretches, deconvolutions))
}

pub(super) fn selected_rc_astro_ids(
    stack_root: &FsPath,
    active_sources: &std::collections::HashMap<String, String>,
) -> std::collections::HashSet<String> {
    let Ok(_guard) = SELECTION_LOCK.lock() else {
        return Default::default();
    };
    let Ok(entries) = std::fs::read_dir(crate::server::storage::stack_processing(stack_root))
    else {
        return Default::default();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let selection = read_selection(&entry.path())?;
            // A ready group may postdate the in-memory snapshot and still
            // precede publication of its whole-job manifest.
            let recently_touched = std::fs::metadata(entry.path())
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|modified| modified.elapsed().ok())
                .is_some_and(|age| age < std::time::Duration::from_secs(24 * 3600));
            if !recently_touched
                && !selection.source.as_ref().is_some_and(|source| {
                    processing_source_exists(stack_root, source, active_sources)
                })
            {
                let _ = std::fs::remove_file(entry.path());
                return None;
            }
            Some(selection)
        })
        .flat_map(|selection| {
            let carried = selection
                .carried
                .filter(|carried| carried.state != CarryState::Stopped)
                .map(|carried| carried.from_stretch_id);
            [selection.selected, carried]
        })
        .flatten()
        .filter(|id| validate_job_id(id).is_ok())
        .filter_map(|id| read_cached_manifest(stack_root, &id))
        .filter_map(|preview| preview.rc_astro_id)
        .collect()
}

fn processing_source_exists(
    stack_root: &FsPath,
    source: &ProcessingSource,
    active_sources: &std::collections::HashMap<String, String>,
) -> bool {
    #[derive(Deserialize)]
    struct SourceManifest {
        artifact_revision: String,
        groups: Vec<SourceGroup>,
    }
    #[derive(Deserialize)]
    struct SourceGroup {
        index: usize,
        state: super::StackGroupState,
    }
    let Some((job_id, group_index)) = source
        .key
        .strip_prefix("mono:")
        .and_then(|key| key.rsplit_once(':'))
    else {
        return false;
    };
    let Ok(group_index) = group_index.parse::<usize>() else {
        return false;
    };
    if validate_job_id(job_id).is_err() {
        return false;
    }
    if active_sources.get(job_id) == Some(&source.revision) {
        return true;
    }
    std::fs::read(super::manifest_path(stack_root, job_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<SourceManifest>(&bytes).ok())
        .is_some_and(|job| {
            job.artifact_revision == source.revision
                && job.groups.get(group_index).is_some_and(|group| {
                    group.index == group_index && group.state == super::StackGroupState::Ready
                })
                && super::fits_path(stack_root, job_id, group_index).is_file()
        })
}

/// The stack a processing request reads: its cache identity, its FITS, and
/// what it shows, for the activity list and the log.
pub(crate) struct StretchSource {
    pub key: String,
    pub revision: String,
    pub path: PathBuf,
    pub scope: super::processing_queue::ProcessingScope,
    /// The stack build and channel group, for the job journal.
    pub job_id: String,
    pub group_index: usize,
    /// What color calibration can use, for a one-shot-color stack.
    pub calibration: Option<super::color_calibration::StackCalibrationSource>,
}

/// How long the request that starts a run waits for its result before it
/// answers `202`. A plain stretch usually finishes within it and answers at
/// once; a run that has to wait for the worker answers `202` straight away.
const INLINE_WAIT: std::time::Duration = std::time::Duration::from_secs(5);

/// What the next poll hears when a person stops a run.
pub(super) const STOPPED_MESSAGE: &str = "Processing was stopped";

/// Refuse a request the run would fail on anyway, before anything is hashed.
fn validate_request(request: &StackViewProcessingRequest) -> Result<(), String> {
    if let Some(deconvolution) = request.deconvolution {
        deconvolution
            .validate()
            .map_err(|error| error.to_string())?;
    }
    if let Some(rc_astro) = &request.rc_astro {
        rc_astro.validate()?;
    }
    if let Some(calibration) = &request.color_calibration {
        calibration.validate()?;
    }
    Ok(())
}

/// A request resolved to its cache identity: what the run does, and the id
/// the browser polls it by.
struct PreparedRun {
    stretch_id: String,
    stretch: StackStretchRequest,
    config: StretchConfig,
    deconvolution: Option<DeconvolutionConfig>,
    deconvolution_id: Option<String>,
    /// Canonical against the installed tools' schemas.
    rc_astro: Option<super::rc_astro::RcAstroProcessing>,
    rc_astro_schemas: Option<Vec<(String, seiza_stacking::ExternalToolSchema)>>,
    /// Per-prefix cache ids for the RC-Astro chain, canonical order.
    rc_astro_chain: Option<Vec<String>>,
    /// The color calibration a one-shot-color stack asks for, spelt out.
    color_calibration_settings: Option<super::color_calibration::StackColorCalibration>,
    color_calibration: Option<super::color_calibration::ColorCalibrationPlan>,
}

impl PreparedRun {
    /// The request as the run uses it, canonical.
    fn request(&self) -> StackViewProcessingRequest {
        StackViewProcessingRequest {
            stretch: self.stretch.clone(),
            deconvolution: self.deconvolution,
            rc_astro: self.rc_astro.clone(),
            color_calibration: self.color_calibration_settings,
        }
    }

    fn operation(&self) -> String {
        super::processing_queue::operation_name(
            self.deconvolution.is_some(),
            self.rc_astro_schemas
                .iter()
                .flatten()
                .map(|(_, schema)| schema.name.as_str()),
        )
    }
}

/// Resolve a request against one stack revision and the installed tools.
/// The same request, sent again or read back from the job journal, comes
/// out under the same id.
async fn prepare_run(
    database_id: &str,
    source_key: &str,
    source_revision: &str,
    calibration_source: Option<&super::color_calibration::StackCalibrationSource>,
    request: StackViewProcessingRequest,
) -> Result<PreparedRun, AppError> {
    let config = request.stretch.config();
    // A one-shot-color stack is calibrated unless the request turns it off;
    // a mono stack never is. The settings, the reference frame's solve and
    // the Gaia catalog are part of the run's identity.
    let color_calibration_settings =
        calibration_source.map(|_| request.color_calibration.unwrap_or_default());
    let color_calibration = calibration_source
        .zip(color_calibration_settings)
        .filter(|(_, settings)| settings.enabled)
        .map(|(source, settings)| source.plan(settings));
    let deconvolution = request.deconvolution;
    let deconvolution_id = deconvolution
        .map(|request| deconvolution_cache_id(database_id, source_key, source_revision, &request))
        .transpose()?;
    // Reading the tool schemas spawns short-lived rc-astro processes; they
    // also pin the CLI and model versions into the cache identity.
    let rc_astro_schemas = match request.rc_astro.clone() {
        Some(config) => Some(
            tokio::task::spawn_blocking(move || super::rc_astro::schemas_for(&config))
                .await
                .map_err(|error| {
                    AppError::InternalError(format!("RC-Astro probe failed: {error}"))
                })?
                .map_err(AppError::BadRequest)?,
        ),
        None => None,
    };
    // Canonicalize before hashing: `1` and `1.0` are one request, and a bad
    // parameter should be refused here, not minutes into the chain.
    let rc_astro = match (&request.rc_astro, &rc_astro_schemas) {
        (Some(config), Some(schemas)) => Some(
            super::rc_astro::normalize_against_schemas(config, schemas)
                .map_err(AppError::BadRequest)?,
        ),
        _ => None,
    };
    let rc_astro_chain = match (&rc_astro, &rc_astro_schemas) {
        (Some(config), Some(schemas)) => Some(super::rc_astro::rc_astro_chain_ids(
            database_id,
            source_key,
            source_revision,
            config,
            schemas,
            deconvolution_id.as_deref(),
        )?),
        _ => None,
    };
    let rc_astro_id = rc_astro_chain.as_ref().and_then(|chain| chain.last());
    let encoded = serde_json::to_vec(&config).map_err(|error| {
        AppError::InternalError(format!("Failed to encode stretch request: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(source_key.as_bytes());
    hasher.update(source_revision.as_bytes());
    hasher.update(STRETCH_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STRETCH_OUTPUT_VERSION.as_bytes());
    hasher.update(&encoded);
    if let Some(deconvolution_id) = &deconvolution_id {
        hasher.update(b"deconvolution");
        hasher.update(deconvolution_id.as_bytes());
    }
    if let Some(rc_astro_id) = rc_astro_id {
        hasher.update(b"rc-astro");
        hasher.update(rc_astro_id.as_bytes());
    }
    // Only a color stack adds these, so a mono stack's runs keep their ids.
    if let Some(settings) = &color_calibration_settings {
        hasher.update(b"color-calibration");
        hasher.update(
            serde_json::to_vec(&(settings, color_calibration.as_ref().map(|plan| &plan.basis)))
                .map_err(|error| {
                    AppError::InternalError(format!("Failed to encode color calibration: {error}"))
                })?,
        );
    }
    let mut stretch_id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut stretch_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(PreparedRun {
        stretch_id,
        stretch: request.stretch,
        config,
        deconvolution,
        deconvolution_id,
        rc_astro,
        rc_astro_schemas,
        rc_astro_chain,
        color_calibration_settings,
        color_calibration,
    })
}

pub(super) async fn apply_to_fits(
    state: Arc<AppState>,
    database_id: String,
    stack_root: PathBuf,
    source: StretchSource,
    request: StackViewProcessingRequest,
    select_processing: bool,
) -> Result<StretchApplyOutcome, AppError> {
    validate_request(&request).map_err(AppError::BadRequest)?;
    let selection_path = selection_path(&stack_root, &source.key, &source.revision);
    let selection_generation = select_processing.then(super::new_artifact_revision);
    if let Some(generation) = &selection_generation {
        request_selection_with_source(
            &selection_path,
            generation,
            Some(ProcessingSource {
                key: source.key.clone(),
                revision: source.revision.clone(),
            }),
            false,
        )
        .map_err(AppError::InternalError)?;
    }
    let prepared = prepare_run(
        &database_id,
        &source.key,
        &source.revision,
        source.calibration.as_ref(),
        request,
    )
    .await?;
    let stretch_id = prepared.stretch_id.clone();
    if let Some(generation) = &selection_generation {
        bind_selection(&selection_path, generation, &stretch_id)
            .map_err(AppError::InternalError)?;
        // A person's own processing replaces a re-apply the server queued
        // for this stack; the same request takes the run over instead.
        super::reapply::replaced_by_person(&state, &source, Some(&stretch_id));
    }
    if let Some(mut cached) = read_cached_manifest(&stack_root, &stretch_id) {
        if cached.request.is_none() {
            cached.request = Some(prepared.request());
            let _guard = SELECTION_LOCK
                .lock()
                .map_err(|error| AppError::InternalError(error.to_string()))?;
            write_json_atomic(&stretch_manifest_path(&stack_root, &stretch_id), &cached)
                .map_err(AppError::InternalError)?;
        }
        if select_processing {
            complete_selection(&selection_path, &stretch_id).map_err(AppError::InternalError)?;
        }
        return Ok(StretchApplyOutcome::Ready(Box::new(cached)));
    }
    // A detached run for this exact request already failed: hand the reason
    // to this poll instead of computing again unasked.
    if let Ok(mut failures) = FAILURES.lock()
        && let Some(message) = failures.remove(&stretch_id)
    {
        return Err(AppError::BadRequest(message));
    }
    let Some(token) = try_begin_in_flight(&stretch_id) else {
        // The same request is already computing; the caller polls.
        return Ok(StretchApplyOutcome::Pending { stretch_id });
    };
    if !select_processing {
        // A server stop, and a restart until the job journal has put the
        // line back, leave a run between two servers. The page keeps
        // polling and finds it again once it is back.
        if state.shutdown.is_stopping() || !super::journal::restored() {
            return Ok(StretchApplyOutcome::Pending { stretch_id });
        }
        return Err(AppError::BadRequest(
            "Processing is no longer running; apply the settings again".into(),
        ));
    }
    if state.shutdown.is_stopping() {
        return Err(AppError::Conflict(
            "The server is stopping; apply the processing again once it is back".into(),
        ));
    }

    let origin = super::journal::JournaledStackJob::Processing {
        database_id: database_id.clone(),
        project_id: source.scope.project_id,
        job_id: source.job_id.clone(),
        group_index: source.group_index,
        revision: source.revision.clone(),
        run_id: stretch_id.clone(),
        request: Box::new(prepared.request()),
        automatic: false,
        attempts: 0,
        running: false,
    };
    // A person is waiting on this: an automatic build in the way yields.
    super::interrupt_automatic(&state);
    let (run, waits) = start_run(
        &state,
        database_id,
        stack_root,
        source,
        selection_path,
        prepared,
        token,
        origin,
    );
    if waits {
        return Ok(StretchApplyOutcome::Pending { stretch_id });
    }
    match tokio::time::timeout(INLINE_WAIT, run).await {
        // Still running: the poll carries its progress.
        Err(_) => Ok(StretchApplyOutcome::Pending { stretch_id }),
        Ok(Ok(Some(response))) => Ok(StretchApplyOutcome::Ready(Box::new(response))),
        Ok(outcome) => {
            if let Ok(mut failures) = FAILURES.lock()
                && let Some(message) = failures.remove(&stretch_id)
            {
                return Err(AppError::BadRequest(message));
            }
            Err(AppError::InternalError(match outcome {
                Err(error) => format!("Stretch worker failed: {error}"),
                Ok(_) => "Stretch rendering failed".into(),
            }))
        }
    }
}

/// Put a prepared run at the back of the line and start its task. The run
/// belongs to the server, not to a request: it keeps going when the request
/// ends, and the client polls with the same request. Returns the task and
/// whether the run has to wait for the worker.
#[allow(clippy::too_many_arguments)]
fn start_run(
    state: &Arc<AppState>,
    database_id: String,
    stack_root: PathBuf,
    source: StretchSource,
    selection_path: PathBuf,
    prepared: PreparedRun,
    token: InFlightToken,
    origin: super::journal::JournaledStackJob,
) -> (tokio::task::JoinHandle<Option<StackStretchPreview>>, bool) {
    let stretch_id = prepared.stretch_id.clone();
    let operation = prepared.operation();
    let label = source.scope.label();
    let identity = StretchIdentity {
        stretch_id: stretch_id.clone(),
        deconvolution_id: prepared.deconvolution_id.clone(),
        rc_astro_chain: prepared.rc_astro_chain.clone(),
        selection_path,
        request: prepared.request(),
    };
    // The run joins the line builds wait in, so the line, not the
    // semaphore, decides when it runs, and a person can move it there.
    let (cancel, waits) = state.stack_previews.queue_processing(
        &stretch_id,
        source.scope,
        operation.clone(),
        prepared.rc_astro.is_some(),
        origin,
    );
    // A server stop waits for the run to stop: a stretch or deconvolution in
    // hand finishes, and an RC-Astro tool is killed and reaped.
    let work = state
        .shutdown
        .hold(format!("{operation} on {label} (db={database_id})"));
    if waits {
        let wait = state
            .stack_previews
            .processing_wait(&stretch_id)
            .map(|wait| wait.describe())
            .unwrap_or_default();
        tracing::info!(
            "Processing {} ({operation} on {label}, db={database_id}) is queued: {wait}",
            short_id(&stretch_id)
        );
    }
    let run = tokio::spawn(compute_stretch_variant(
        Arc::clone(state),
        database_id,
        stack_root,
        ProcessingTicket {
            token,
            cancel,
            work,
        },
        identity,
        source.path,
        prepared.config,
        prepared.deconvolution,
        prepared.rc_astro,
        prepared.rc_astro_schemas,
        prepared.color_calibration,
    ));
    (run, waits)
}

/// Put back a processing run the job journal kept, in the line and under the
/// id the browser polls with, so a page left open across the restart finds
/// it. Steps of its RC-Astro chain that finished before the restart come from
/// the cache.
pub(super) async fn restore_run(
    state: &Arc<AppState>,
    origin: super::journal::JournaledStackJob,
) -> Result<(), super::journal::NotRestored> {
    let result = restore_processing(state, origin.clone()).await;
    // A re-apply that does not come back is settled as failed, so its
    // card stops waiting and no recovery revives it.
    if let Err(super::journal::NotRestored::Skip(reason)) = &result {
        super::reapply::give_up(state, &origin, reason);
    }
    result
}

async fn restore_processing(
    state: &Arc<AppState>,
    origin: super::journal::JournaledStackJob,
) -> Result<(), super::journal::NotRestored> {
    use super::journal::{JournaledStackJob, NotRestored};
    let JournaledStackJob::Processing {
        database_id,
        project_id,
        job_id,
        group_index,
        revision,
        run_id,
        request,
        automatic,
        attempts,
        running,
    } = origin
    else {
        return Err(NotRestored::Skip("it is not a processing run".into()));
    };
    let ctx = state
        .get_database(&database_id)
        .ok_or_else(|| NotRestored::Skip(format!("database {database_id} is gone")))?;
    let source = match super::stack_processing_source(state, &ctx, &job_id, group_index) {
        Ok(source) => source,
        // No stack folder at all: its disk may not be mounted yet.
        Err(_) if !ctx.stack_root.exists() => {
            return Err(NotRestored::Retry(format!(
                "the stack folder {} is not there",
                ctx.stack_root.display()
            )));
        }
        Err(_) => {
            return Err(NotRestored::Skip(format!(
                "its stack {} is gone",
                short_id(&job_id)
            )));
        }
    };
    if source.revision != revision {
        return Err(NotRestored::Skip(format!(
            "its stack {} was rebuilt",
            short_id(&job_id)
        )));
    }
    // A person's re-apply is journaled as theirs; its carry knows the run.
    let reapply = automatic || carries_run(&ctx.stack_root, &source.key, &source.revision, &run_id);
    // A re-apply only ever runs on the newest revision of its channel.
    if reapply && !super::reapply::is_latest(&ctx, project_id, &source) {
        return Err(NotRestored::Skip(format!(
            "a newer stack replaced {}",
            short_id(&job_id)
        )));
    }
    validate_request(&request).map_err(NotRestored::Skip)?;
    let prepared = prepare_run(
        &ctx.id,
        &source.key,
        &source.revision,
        source.calibration.as_ref(),
        *request,
    )
    .await
    .map_err(|error| NotRestored::Skip(format!("{error:?}")))?;
    let stretch_id = prepared.stretch_id.clone();
    if stretch_id != run_id {
        tracing::info!(
            "Processing {} comes back as {}: the installed tools changed",
            short_id(&run_id),
            short_id(&stretch_id)
        );
    }
    let selection_path = selection_path(&ctx.stack_root, &source.key, &source.revision);
    if read_cached_manifest(&ctx.stack_root, &stretch_id).is_some() {
        complete_selection(&selection_path, &stretch_id).map_err(NotRestored::Skip)?;
        return Err(NotRestored::Skip(format!(
            "{} finished before the restart",
            short_id(&stretch_id)
        )));
    }
    let token = try_begin_in_flight(&stretch_id).ok_or_else(|| {
        NotRestored::Skip(format!("{} is already running", short_id(&stretch_id)))
    })?;
    let origin = JournaledStackJob::Processing {
        database_id: ctx.id.clone(),
        project_id,
        job_id,
        group_index,
        revision,
        run_id: stretch_id.clone(),
        request: Box::new(prepared.request()),
        automatic,
        attempts,
        running,
    };
    if reapply {
        super::reapply::restored(
            state,
            &ctx.id,
            project_id,
            &source,
            prepared.request(),
            !automatic,
            &stretch_id,
        );
    }
    tracing::info!(
        "Processing {} ({} on {}) is back in the line after the restart",
        short_id(&stretch_id),
        prepared.operation(),
        source.scope.label()
    );
    // The selection on disk already says which run a person asked for last;
    // a restored run leaves it alone and takes it only if it is that one.
    start_run(
        state,
        ctx.id.clone(),
        ctx.stack_root.clone(),
        source,
        selection_path,
        prepared,
        token,
        origin,
    );
    Ok(())
}

/// How starting a re-apply went.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ReapplyStart {
    /// Queued as this run.
    Queued(String),
    /// The result already existed and is now the revision's selection.
    Done,
    /// No longer wanted: a person applied or reverted processing, or a newer
    /// stack replaced this one, while the tools were probed.
    Withdrawn,
}

/// Queue a re-apply: the request the previous revision showed, run on the
/// new revision in the build line. An automatic one joins the back as
/// automatic work; a `personal` one, carried from a person's build, is
/// theirs and goes to the front. `still_wanted` is asked again once the
/// tools are probed, under the selection lock, together with whether the
/// revision's carry still waits: whatever a person did meanwhile wins.
#[allow(clippy::too_many_arguments)]
pub(super) async fn start_reapply(
    state: &Arc<AppState>,
    ctx: &super::DatabaseContext,
    job_id: &str,
    group_index: usize,
    revision: &str,
    request: StackViewProcessingRequest,
    personal: bool,
    still_wanted: &(dyn Fn() -> bool + Send + Sync),
) -> Result<ReapplyStart, String> {
    let source = super::stack_processing_source(state, ctx, job_id, group_index)
        .map_err(|error| format!("its stack is not readable: {error:?}"))?;
    if source.revision != revision {
        return Err("the stack was rebuilt meanwhile".into());
    }
    validate_request(&request)?;
    let prepared = prepare_run(
        &ctx.id,
        &source.key,
        &source.revision,
        source.calibration.as_ref(),
        request,
    )
    .await
    .map_err(|error| match error {
        AppError::BadRequest(message) | AppError::InternalError(message) => message,
        other => format!("{other:?}"),
    })?;
    let stretch_id = prepared.stretch_id.clone();
    let selection_path = selection_path(&ctx.stack_root, &source.key, &source.revision);
    // Asked for on this revision, as a person's Apply would be, so the run
    // becomes the selection when it lands; the carried result stays until
    // then.
    if !claim_carry(
        &selection_path,
        &super::new_artifact_revision(),
        ProcessingSource {
            key: source.key.clone(),
            revision: source.revision.clone(),
        },
        &stretch_id,
        still_wanted,
    )? {
        return Ok(ReapplyStart::Withdrawn);
    }
    if read_cached_manifest(&ctx.stack_root, &stretch_id).is_some() {
        complete_selection(&selection_path, &stretch_id)?;
        update_carry(&selection_path, |_| ())?;
        return Ok(ReapplyStart::Done);
    }
    let Some(token) = try_begin_in_flight(&stretch_id) else {
        // A person's run of this very request is under way: it is the
        // re-apply.
        return Ok(ReapplyStart::Queued(stretch_id));
    };
    let origin = super::journal::JournaledStackJob::Processing {
        database_id: ctx.id.clone(),
        project_id: source.scope.project_id,
        job_id: source.job_id.clone(),
        group_index: source.group_index,
        revision: source.revision.clone(),
        run_id: stretch_id.clone(),
        request: Box::new(prepared.request()),
        automatic: !personal,
        attempts: 0,
        running: false,
    };
    tracing::info!(
        "Processing {} ({} on {}, db={}) re-applies the previous stack's processing",
        short_id(&stretch_id),
        prepared.operation(),
        source.scope.label(),
        ctx.id
    );
    start_run(
        state,
        ctx.id.clone(),
        ctx.stack_root.clone(),
        source,
        selection_path,
        prepared,
        token,
        origin,
    );
    // Next after the work in hand, ahead of the person's other builds.
    if personal {
        state.stack_previews.move_in_line(&stretch_id, 0);
    }
    Ok(ReapplyStart::Queued(stretch_id))
}

/// Claim a revision's carried re-apply for one run, in one go under the
/// selection lock: only while nothing is selected, the carry still waits, and
/// the caller still wants it. A person's Apply clears the carry and a Revert
/// stops it, so either one made meanwhile is kept.
fn claim_carry(
    selection_path: &FsPath,
    generation: &str,
    source: ProcessingSource,
    stretch_id: &str,
    still_wanted: &(dyn Fn() -> bool + Send + Sync),
) -> Result<bool, String> {
    let _guard = SELECTION_LOCK.lock().map_err(|error| error.to_string())?;
    let Some(mut selection) = read_selection(selection_path) else {
        return Ok(false);
    };
    let waiting = selection.selected.is_none()
        && selection
            .carried
            .as_ref()
            .is_some_and(|carried| carried.state == CarryState::Waiting);
    if !waiting || !still_wanted() {
        return Ok(false);
    }
    selection.requested = generation.into();
    selection.desired = Some(stretch_id.into());
    selection.source = Some(source);
    if let Some(carried) = selection.carried.as_mut() {
        carried.run_id = Some(stretch_id.into());
    }
    write_json_atomic(selection_path, &selection)?;
    Ok(true)
}

/// Carry the processing a stack's previous revision shows to its new
/// revision, unless the new revision already has processing of its own or
/// has carried once before. Returns the request to run again and the result
/// shown meanwhile.
pub(super) fn carry_processing(
    stack_root: &FsPath,
    previous_key: &str,
    previous_revision: &str,
    current_key: &str,
    current_revision: &str,
    personal: bool,
) -> Result<Option<(StackViewProcessingRequest, String)>, String> {
    let _guard = SELECTION_LOCK.lock().map_err(|error| error.to_string())?;
    let current_path = selection_path(stack_root, current_key, current_revision);
    if let Some(current) = read_selection(&current_path)
        && (current.selected.is_some()
            || !current.requested.is_empty()
            || current.carried.is_some())
    {
        return Ok(None);
    }
    let Some(previous) =
        read_selection(&selection_path(stack_root, previous_key, previous_revision))
    else {
        return Ok(None);
    };
    // What the previous card shows: its own result, or one it carried and
    // has not replaced yet.
    let shown = previous.selected.or_else(|| {
        previous
            .carried
            .filter(|carried| carried.state != CarryState::Stopped)
            .map(|carried| carried.from_stretch_id)
    });
    let Some(shown) = shown.filter(|id| validate_job_id(id).is_ok()) else {
        return Ok(None);
    };
    let Some(request) = read_cached_manifest(stack_root, &shown).and_then(|shown| shown.request)
    else {
        return Ok(None);
    };
    write_json_atomic(
        &current_path,
        &ProcessingSelection {
            selected: None,
            requested: String::new(),
            desired: None,
            source: Some(ProcessingSource {
                key: current_key.into(),
                revision: current_revision.into(),
            }),
            carried: Some(CarriedProcessing {
                from_stretch_id: shown.clone(),
                from_revision: previous_revision.into(),
                state: CarryState::Waiting,
                personal,
                run_id: None,
                error: None,
            }),
        },
    )?;
    Ok(Some((request, shown)))
}

/// Change what a revision's carried result says. Nothing to change is no
/// error.
fn update_carry(
    selection_path: &FsPath,
    change: impl FnOnce(&mut CarriedProcessing),
) -> Result<(), String> {
    let _guard = SELECTION_LOCK.lock().map_err(|error| error.to_string())?;
    let Some(mut selection) = read_selection(selection_path) else {
        return Ok(());
    };
    let Some(carried) = selection.carried.as_mut() else {
        return Ok(());
    };
    change(carried);
    // Once the revision shows its own result, nothing carried is left.
    if selection.selected.is_some() {
        selection.carried = None;
    }
    write_json_atomic(selection_path, &selection)
}

/// The re-apply landed: the revision shows its own result now.
pub(super) fn carry_published(
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
) -> Result<(), String> {
    update_carry(
        &selection_path(stack_root, source_key, source_revision),
        |_| (),
    )
}

/// The re-apply's run stepped aside: it waits again, with no run.
pub(super) fn clear_carry_run(
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
) -> Result<(), String> {
    update_carry(
        &selection_path(stack_root, source_key, source_revision),
        |carried| carried.run_id = None,
    )
}

/// Settle a revision's carried re-apply: failed with a reason, or stopped.
pub(super) fn end_carry(
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
    state: CarryState,
    error: Option<String>,
) -> Result<(), String> {
    update_carry(
        &selection_path(stack_root, source_key, source_revision),
        |carried| {
            carried.state = state;
            carried.error = error;
        },
    )
}

/// A revision whose processing is still carried from the previous one and
/// waiting for its re-apply, as a restarted server finds it on disk.
pub(super) struct WaitingCarry {
    pub key: String,
    pub revision: String,
    pub request: StackViewProcessingRequest,
    pub personal: bool,
}

pub(super) fn waiting_carries(stack_root: &FsPath) -> Vec<WaitingCarry> {
    let Ok(_guard) = SELECTION_LOCK.lock() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(crate::server::storage::stack_processing(stack_root))
    else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| read_selection(&entry.path()))
        .filter(|selection| selection.selected.is_none())
        .filter_map(|selection| {
            let carried = selection.carried?;
            let source = selection.source?;
            (carried.state == CarryState::Waiting).then_some(())?;
            let request = read_cached_manifest(stack_root, &carried.from_stretch_id)?.request?;
            Some(WaitingCarry {
                key: source.key,
                revision: source.revision,
                request,
                personal: carried.personal,
            })
        })
        .collect()
}

/// Whether a revision's carried processing still waits on this run: the run
/// is its re-apply.
pub(super) fn carries_run(
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
    run_id: &str,
) -> bool {
    let Ok(_guard) = SELECTION_LOCK.lock() else {
        return false;
    };
    read_selection(&selection_path(stack_root, source_key, source_revision))
        .and_then(|selection| selection.carried)
        .is_some_and(|carried| {
            carried.state == CarryState::Waiting && carried.run_id.as_deref() == Some(run_id)
        })
}

/// The processing a revision shows: its own, or the previous revision's
/// while the server runs it again here, marked as such.
pub(super) fn selected_or_carried(
    manager: &super::StackPreviewManager,
    stack_root: &FsPath,
    source_key: &str,
    source_revision: &str,
) -> Result<Option<StackStretchPreview>, AppError> {
    if let Some(selected) = selected_processing(stack_root, source_key, source_revision)? {
        return Ok(Some(selected));
    }
    let carried = {
        let _guard = SELECTION_LOCK
            .lock()
            .map_err(|error| AppError::InternalError(error.to_string()))?;
        read_selection(&selection_path(stack_root, source_key, source_revision))
            .and_then(|selection| selection.carried)
    };
    let Some(carried) = carried.filter(|carried| carried.state != CarryState::Stopped) else {
        return Ok(None);
    };
    let Some(mut shown) = read_cached_manifest(stack_root, &carried.from_stretch_id) else {
        return Ok(None);
    };
    let state = match carried.state {
        CarryState::Failed => ReprocessingState::Failed,
        _ => match carried
            .run_id
            .as_deref()
            .and_then(|run_id| manager.processing_state(run_id))
        {
            Some(super::StackJobState::Running) => ReprocessingState::Running,
            Some(super::StackJobState::Queued) => ReprocessingState::Queued,
            _ => ReprocessingState::Waiting,
        },
    };
    shown.reprocessing = Some(Reprocessing {
        state,
        from_revision: carried.from_revision,
        run_id: carried.run_id,
        error: carried.error,
    });
    Ok(Some(shown))
}

/// The first twelve characters of a run id, enough to tell runs apart in
/// the log.
fn short_id(stretch_id: &str) -> &str {
    &stretch_id[..stretch_id.len().min(12)]
}

/// What a detached run holds until it ends: its in-flight entry, its stop
/// flag, and the server stop's wait.
struct ProcessingTicket {
    token: InFlightToken,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    work: crate::server::shutdown::WorkGuard,
}

/// Takes a run out of the activity list and the line when it ends, however
/// it ends.
struct ProcessingRegistration {
    state: Arc<AppState>,
    run_id: String,
}

impl Drop for ProcessingRegistration {
    fn drop(&mut self) {
        self.state.stack_previews.finish_processing(&self.run_id);
    }
}

/// When a running run stops. A person's stop and a server stop both kill an
/// RC-Astro tool at once, and its exchange folder goes with it; steps that
/// finished stay cached. A stretch or deconvolution in hand finishes; the
/// run stops at its next check, before deconvolution or before each tool.
pub(super) struct RunStop {
    cancel: Arc<std::sync::atomic::AtomicBool>,
    shutdown: Arc<crate::server::shutdown::Shutdown>,
}

impl RunStop {
    fn stopped(&self) -> bool {
        self.cancel.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// A person's stop, not a server's.
    fn stopped_by_a_person(&self) -> bool {
        self.stopped() && !self.shutdown.is_stopping()
    }

    /// Read before each costly step.
    fn check(&self) -> Result<(), String> {
        if self.shutdown.is_stopping() {
            return Err("The server is stopping; the run resumes after the restart".into());
        }
        if self.stopped() {
            return Err(STOPPED_MESSAGE.into());
        }
        Ok(())
    }

    /// What a person reads when a step failed: the stop they asked for, not
    /// the tool's own complaint about being killed.
    fn explain(&self, message: String) -> String {
        if self.stopped_by_a_person() {
            STOPPED_MESSAGE.into()
        } else {
            message
        }
    }

    /// Kills a running RC-Astro tool when the run is stopped.
    fn signal(&self) -> seiza_stacking::CancelSignal {
        seiza_stacking::CancelSignal::from(Arc::clone(&self.cancel))
    }
}

/// Wait for the worker in the line, render, and publish the manifest. Runs
/// to completion regardless of the HTTP request that started it. A failure
/// is parked in [`FAILURES`] for the next poll, except one a server stop
/// caused: the job journal brings that run back after the restart. Returns
/// the response on success so the request that started it can answer
/// directly.
#[allow(clippy::too_many_arguments)]
async fn compute_stretch_variant(
    state: Arc<AppState>,
    database_id: String,
    stack_root: PathBuf,
    ticket: ProcessingTicket,
    identity: StretchIdentity,
    source_path: PathBuf,
    config: StretchConfig,
    deconvolution: Option<DeconvolutionConfig>,
    rc_astro: Option<super::rc_astro::RcAstroProcessing>,
    rc_astro_schemas: Option<Vec<(String, seiza_stacking::ExternalToolSchema)>>,
    color_calibration: Option<super::color_calibration::ColorCalibrationPlan>,
) -> Option<StackStretchPreview> {
    let ProcessingTicket {
        token,
        cancel,
        work,
    } = ticket;
    // Dropped in reverse: the run leaves the activity list, then the
    // in-flight set, and the server stop's wait ends last, once every file
    // is written or the tool is reaped.
    let _work = work;
    let _token = token;
    let stretch_id = identity.stretch_id.clone();
    let _registration = ProcessingRegistration {
        state: Arc::clone(&state),
        run_id: stretch_id.clone(),
    };
    let short = short_id(&stretch_id).to_string();
    let summary = state
        .stack_previews
        .processing_summary(&stretch_id)
        .unwrap_or_else(|| "processing".into());
    // `by_stop`: the failure came from a stop. One a server stop caused is
    // no failure: the run comes back from the journal, and a page still
    // polling must not hear that it failed. A re-apply the server queued
    // has nobody polling: its card hears through its selection instead.
    let park_failure = |message: String, by_stop: bool| {
        let outcome = if by_stop && state.shutdown.is_stopping() {
            tracing::info!(
                "Processing {short} ({summary}) stopped for the server stop; it resumes after \
                 the restart"
            );
            super::reapply::RunOutcome::Interrupted
        } else {
            tracing::warn!("Stack stretch {stretch_id} failed: {message}");
            if !state.stack_previews.is_automatic_processing(&stretch_id)
                && let Ok(mut failures) = FAILURES.lock()
            {
                failures.insert(stretch_id.clone(), message.clone());
            }
            if by_stop {
                super::reapply::RunOutcome::Stopped
            } else {
                super::reapply::RunOutcome::Failed(message)
            }
        };
        super::reapply::run_finished(&state, &stretch_id, outcome);
    };
    let queued_at = std::time::Instant::now();
    let permit = state.stack_previews.wait_turn(&stretch_id, &cancel).await;
    // A stop that lands as the worker frees up still keeps the run from
    // starting.
    let Some(permit) =
        permit.filter(|_| state.stack_previews.start_processing(&stretch_id, &cancel))
    else {
        tracing::info!("Processing {short} ({summary}) left the line");
        park_failure(STOPPED_MESSAGE.into(), true);
        return None;
    };
    tracing::info!(
        "Processing {short} ({summary}) starts after waiting {:.1} s",
        queued_at.elapsed().as_secs_f64()
    );
    if let Some(cached) = read_cached_manifest(&stack_root, &identity.stretch_id) {
        if let Err(message) = complete_selection(&identity.selection_path, &identity.stretch_id) {
            park_failure(message, false);
            return None;
        }
        state.stack_previews.processing_completed(&stretch_id);
        super::reapply::run_finished(&state, &stretch_id, super::reapply::RunOutcome::Published);
        return Some(cached);
    }
    // A person waiting on it outranks background work; a re-apply is
    // background work itself.
    let automatic = state.stack_previews.is_automatic_processing(&stretch_id);
    let guard = (!automatic).then(|| state.begin_named_interactive_job("stack processing"));
    let priority = if automatic {
        crate::concurrency::Priority::Background
    } else {
        crate::concurrency::Priority::Interactive
    };
    let stop = Arc::new(RunStop {
        cancel,
        shutdown: Arc::clone(&state.shutdown),
    });
    let stop_for_render = Arc::clone(&stop);
    let state_for_render = Arc::clone(&state);
    let stack_for_render = stack_root.clone();
    let database_for_render = database_id.clone();
    let started = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let _permit = permit;
        let stop = stop_for_render;
        let rendered = render_fits_variant(
            &state_for_render,
            &stack_for_render,
            &identity.stretch_id,
            &source_path,
            &config,
            deconvolution,
            identity.deconvolution_id.as_deref(),
            rc_astro,
            identity.rc_astro_chain.as_deref(),
            rc_astro_schemas,
            color_calibration.as_ref(),
            &stop,
            priority,
        )
        .map_err(|message| stop.explain(message))?;
        // Stopped by a person while it rendered: publish nothing they asked
        // to stop. A server stop lets a finished render through.
        if stop.stopped_by_a_person() {
            return Err(STOPPED_MESSAGE.to_string());
        }
        let response = build_stretch_preview(
            &database_for_render,
            &identity,
            deconvolution.is_some(),
            rendered,
        );
        write_json_atomic(
            &stretch_manifest_path(&stack_for_render, &identity.stretch_id),
            &response,
        )?;
        complete_selection(&identity.selection_path, &identity.stretch_id)?;
        Ok::<_, String>(response)
    })
    .await;
    match result {
        Ok(Ok(response)) => {
            tracing::info!(
                "Processing {short} ({summary}) finished in {:.1} s",
                started.elapsed().as_secs_f64()
            );
            state.stack_previews.processing_completed(&stretch_id);
            super::reapply::run_finished(
                &state,
                &stretch_id,
                super::reapply::RunOutcome::Published,
            );
            Some(response)
        }
        Ok(Err(message)) => {
            park_failure(message, stop.stopped());
            None
        }
        Err(join_error) => {
            park_failure(format!("Stretch worker failed: {join_error}"), false);
            None
        }
    }
}

fn build_stretch_preview(
    database_id: &str,
    identity: &StretchIdentity,
    deconvolution_requested: bool,
    rendered: RenderedVariant,
) -> StackStretchPreview {
    let stretch_id = &identity.stretch_id;
    let has_stars = rendered
        .rc_astro
        .as_ref()
        .is_some_and(|result| result.has_stars);
    StackStretchPreview {
        schema_version: 2,
        stretch_id: stretch_id.clone(),
        stretch_version: SEIZA_STRETCH_OUTPUT_VERSION.into(),
        deconvolution_version: rendered
            .deconvolution
            .as_ref()
            .map(|_| deconvolution_version()),
        deconvolution_id: identity.deconvolution_id.clone(),
        config: rendered.config,
        request: Some(identity.request.clone()),
        resolved_plan: rendered.resolved_plan,
        source_transfer: rendered.source_transfer,
        input_range: rendered.input_range,
        linked_statistics: rendered.linked_statistics,
        channel_statistics: rendered.channel_statistics,
        luminance_statistics: rendered.luminance_statistics,
        deconvolution: rendered.deconvolution,
        rc_astro: rendered.rc_astro,
        rc_astro_id: identity.rc_astro_id().map(str::to_string),
        color_calibration: rendered.color_calibration,
        color_calibration_basis: rendered.color_calibration_basis,
        color_calibration_outdated: None,
        preview_url: format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/preview"),
        original_preview_url: format!(
            "/api/db/{database_id}/stack-previews/stretch/{stretch_id}/preview?size=original"
        ),
        fits_url: (deconvolution_requested || identity.rc_astro_id().is_some())
            .then(|| format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/fits")),
        stars_preview_url: has_stars
            .then(|| format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/stars")),
        stars_original_preview_url: has_stars.then(|| {
            format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/stars?size=original")
        }),
        stars_fits_url: has_stars.then(|| {
            format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/stars-fits")
        }),
        reprocessing: None,
    }
}

struct RenderedVariant {
    config: StretchConfig,
    resolved_plan: serde_json::Value,
    source_transfer: StackStretchSourceTransfer,
    input_range: Option<StackStretchInputRange>,
    linked_statistics: RobustStatistics,
    channel_statistics: Vec<Option<RobustStatistics>>,
    luminance_statistics: Option<RobustStatistics>,
    deconvolution: Option<StackDeconvolutionResult>,
    rc_astro: Option<super::rc_astro::StackRcAstroResult>,
    color_calibration: Option<super::color_calibration::StackColorCalibrationOutcome>,
    color_calibration_basis: Option<super::color_calibration::StackColorCalibrationBasis>,
}

#[allow(clippy::too_many_arguments)]
fn render_fits_variant(
    state: &Arc<AppState>,
    stack_root: &FsPath,
    stretch_id: &str,
    source_path: &FsPath,
    config: &StretchConfig,
    deconvolution_request: Option<DeconvolutionConfig>,
    deconvolution_id: Option<&str>,
    rc_astro_request: Option<super::rc_astro::RcAstroProcessing>,
    rc_astro_chain: Option<&[String]>,
    rc_astro_schemas: Option<Vec<(String, seiza_stacking::ExternalToolSchema)>>,
    color_calibration_plan: Option<&super::color_calibration::ColorCalibrationPlan>,
    stop: &RunStop,
    priority: crate::concurrency::Priority,
) -> Result<RenderedVariant, String> {
    report_stage(stretch_id, "Reading the stack");
    let frame =
        crate::image_io::open_linear_frame(source_path).map_err(|error| error.to_string())?;
    let samples = frame.image.data.len();
    let bytes_per_sample = if deconvolution_request.is_some() || rc_astro_chain.is_some() {
        DECONVOLUTION_BYTES_PER_SAMPLE
    } else {
        STRETCH_BYTES_PER_SAMPLE
    };
    let estimate = (samples as u64).saturating_mul(bytes_per_sample);
    let policy = state.worker_policy();
    if let Some(available) = crate::concurrency::available_memory_bytes()
        && estimate > (available as f64 * policy.memory_budget_fraction) as u64
    {
        return Err(format!(
            "Estimated stretch memory {} MiB exceeds the configured available-memory budget",
            estimate / (1024 * 1024)
        ));
    }
    let budget =
        crate::concurrency::plan_workers(None, &policy, priority, Some(frame.image.pixel_count()));
    let lease = state.lease_workers(priority, budget.workers);
    let pool = crate::concurrency::ComputePool::take("stack-stretch", lease.workers)
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Stack stretch {stretch_id}: {} worker(s) — {}; {}",
        lease.workers,
        budget.rationale,
        lease.summary()
    );
    let source_transfer = frame
        .headers
        .iter()
        .find(|(keyword, _)| keyword.eq_ignore_ascii_case("SEIZATRF"))
        .and_then(|(_, value)| value.as_str())
        .filter(|value| value.eq_ignore_ascii_case("DISPLAY"))
        .map(|_| StackStretchSourceTransfer::DisplayReferred)
        .unwrap_or(StackStretchSourceTransfer::Linear);
    let source_analysis = pool
        .install(|| {
            StretchAnalysis::analyze(
                &frame.image.data,
                frame.image.channels,
                config.max_analysis_samples,
            )
        })
        .map_err(|error| error.to_string())?;
    if (deconvolution_request.is_some() || rc_astro_request.is_some())
        && source_transfer == StackStretchSourceTransfer::DisplayReferred
    {
        return Err("Linear-light processing requires a linear stack source".into());
    }
    let mut processed = None;
    let mut cached_processed = None;
    let deconvolution = if let Some(request) = deconvolution_request {
        let deconvolution_id = deconvolution_id
            .ok_or_else(|| "Deconvolution cache identity is missing".to_string())?;
        // In use from now: the janitor keeps a deconvolution a week past its
        // last use, and the stretch that will name it has no manifest yet.
        touch_dir(&deconvolution_dir(stack_root, deconvolution_id));
        let fits = deconvolution_fits_path(stack_root, deconvolution_id);
        let manifest = deconvolution_manifest_path(stack_root, deconvolution_id);
        let cached = std::fs::read(&manifest)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<CachedDeconvolution>(&bytes).ok())
            .filter(|cached| {
                cached.schema_version == DECONVOLUTION_CACHE_VERSION
                    && cached.algorithm_version == ALGORITHM_VERSION
                    && cached.deconvolution_id == deconvolution_id
                    && cached.config == request
                    && fits.is_file()
            });
        if let Some(cached) = cached {
            cached_processed =
                Some(crate::image_io::open_linear_frame(&fits).map_err(|error| error.to_string())?);
            Some(StackDeconvolutionResult {
                config: cached.config,
                channels: cached.channels,
            })
        } else {
            stop.check()?;
            report_stage(stretch_id, "Deconvolution");
            let (restored, result) = pool.install(|| apply_deconvolution(&frame.image, request))?;
            let parent = fits
                .parent()
                .ok_or_else(|| "Processed stack FITS path has no parent".to_string())?;
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            let temporary = fits.with_extension(format!("{}.tmp.fits", std::process::id()));
            write_processed_image_fits_f32(
                &temporary,
                &restored,
                &frame.headers,
                &deconvolution_cards(request),
            )
            .map_err(|error| error.to_string())?;
            std::fs::rename(&temporary, &fits).map_err(|error| error.to_string())?;
            write_json_atomic(
                &manifest,
                &CachedDeconvolution {
                    schema_version: DECONVOLUTION_CACHE_VERSION,
                    deconvolution_id: deconvolution_id.into(),
                    algorithm_version: ALGORITHM_VERSION,
                    config: request,
                    channels: result.channels.clone(),
                },
            )?;
            processed = Some(restored);
            Some(result)
        }
    } else {
        None
    };
    let linear = processed
        .as_ref()
        .or_else(|| cached_processed.as_ref().map(|frame| &frame.image))
        .unwrap_or(&frame.image);
    // RC-Astro runs after deconvolution: the tools operate on whatever the
    // linear image has become by this point.
    let mut rc_astro_outcome = None;
    if let (Some(rc_astro_config), Some(chain_ids), Some(schemas)) =
        (rc_astro_request, rc_astro_chain, rc_astro_schemas.as_ref())
    {
        // The tools are programs of their own and choose their own
        // threads; the analysis after them runs in this job's pool. A
        // person's stop kills the tool in hand; a server stop is read only
        // between tools, so the one in hand finishes and its result stays.
        let cancel = stop.signal();
        rc_astro_outcome = Some(pool.install(|| {
            super::rc_astro::apply_rc_astro(
                stack_root,
                chain_ids,
                &rc_astro_config,
                schemas,
                linear,
                &frame.headers,
                &mut |stage, fraction| report_progress(stretch_id, stage, fraction),
                Some(&cancel),
                &mut || stop.check(),
            )
        })?);
    }
    let linear = rc_astro_outcome
        .as_ref()
        .map(|outcome| &outcome.image)
        .unwrap_or(linear);
    // A one-shot-color stack is calibrated on its linear pixels, after
    // restoration and before the stretch.
    let mut calibrated = None;
    let color_calibration = match color_calibration_plan {
        Some(plan) if source_transfer == StackStretchSourceTransfer::Linear => {
            stop.check()?;
            report_stage(stretch_id, "Color calibration");
            let open_catalog = || {
                state
                    .astrometry
                    .gaia_photometry_catalog()
                    .map(|(catalog, _)| catalog)
            };
            let (image, mut outcome) = pool
                .install(|| super::color_calibration::calibrate_stack(plan, linear, &open_catalog));
            // An unlinked stretch pulls each channel to its own median.
            if config.color_strategy == seiza_stretch::ColorStrategy::Unlinked {
                super::color_calibration::mark_stretched_separately(&mut outcome);
            }
            calibrated = image;
            Some(outcome)
        }
        _ => None,
    };
    let linear = calibrated.as_ref().unwrap_or(linear);
    report_stage(stretch_id, "Rendering preview");
    let normalized = if source_transfer == StackStretchSourceTransfer::Linear {
        Some(normalize_linear_image(linear)?)
    } else {
        None
    };
    let input_range = normalized.as_ref().map(|(_, range)| *range);
    let prepared = normalized
        .as_ref()
        .map(|(image, _)| image)
        .unwrap_or(linear);
    let screen = stretch_preview_path(stack_root, stretch_id);
    let original = stretch_original_preview_path(stack_root, stretch_id);
    let render = pool.install(|| {
        render_image_previews_with_details(prepared, config, &screen, &original, |_| {})
    })?;
    // The stars image shares the starless image's display transform —
    // normalization range and resolved curves alike — so the two previews
    // compose: screening the stars preview over the starless one
    // approximates the un-separated stack. Analyzing the stars image on its
    // own would instead auto-stretch its near-empty background into noise.
    if let Some(outcome) = &rc_astro_outcome
        && let Some(stars) = &outcome.stars
    {
        let stars_prepared = match &input_range {
            Some(range) => {
                let span = range.white - range.black;
                let data = stars
                    .data
                    .iter()
                    .map(|value| {
                        if value.is_finite() {
                            (*value - range.black) / span
                        } else {
                            f32::NAN
                        }
                    })
                    .collect();
                Some(
                    LinearImage::new(stars.width, stars.height, stars.channels, data)
                        .map_err(|error| error.to_string())?,
                )
            }
            None => None,
        };
        let stars_source = stars_prepared.as_ref().unwrap_or(stars);
        let stars_screen = stretch_stars_preview_path(stack_root, stretch_id);
        let stars_original = stretch_stars_original_preview_path(stack_root, stretch_id);
        pool.install(|| {
            render_previews_with_plan(stars_source, &render.plan, &stars_screen, &stars_original)
        })?;
    }
    Ok(RenderedVariant {
        config: config.clone(),
        resolved_plan: serde_json::to_value(render.plan).map_err(|error| error.to_string())?,
        source_transfer,
        input_range,
        linked_statistics: source_analysis.linked_statistics(),
        channel_statistics: source_analysis.channel_statistics(),
        luminance_statistics: source_analysis.luminance_statistics(),
        deconvolution,
        rc_astro: rc_astro_outcome.map(|outcome| outcome.result),
        color_calibration_basis: color_calibration
            .is_some()
            .then(|| color_calibration_plan.map(|plan| plan.basis.clone()))
            .flatten(),
        color_calibration,
    })
}

pub(super) fn apply_deconvolution(
    image: &LinearImage,
    request: DeconvolutionConfig,
) -> Result<(LinearImage, StackDeconvolutionResult), String> {
    let result = deconvolve_masked(
        &image.data,
        image.width,
        image.height,
        image.channels,
        &request,
    )
    .map_err(|error| error.to_string())?;
    let diagnostics = result.channels;
    let restored = LinearImage::new(image.width, image.height, image.channels, result.data)
        .map_err(|error| error.to_string())?;
    Ok((
        restored,
        StackDeconvolutionResult {
            config: request,
            channels: diagnostics,
        },
    ))
}

fn deconvolution_cards(request: DeconvolutionConfig) -> Vec<WriteHeaderCard> {
    vec![
        WriteHeaderCard::new("SEIZADC", HeaderValue::String("RL-GAUSS".into()))
            .with_comment("Seiza deconvolution method"),
        WriteHeaderCard::new("SEIZATRF", HeaderValue::String("LINEAR".into()))
            .with_comment("linear sample transfer"),
        WriteHeaderCard::new(
            "DCFWHM",
            HeaderValue::Float(f64::from(request.psf_fwhm_pixels)),
        )
        .with_comment("Gaussian PSF FWHM in pixels"),
        WriteHeaderCard::new(
            "DCITER",
            HeaderValue::Integer(i64::try_from(request.iterations).unwrap_or(i64::MAX)),
        )
        .with_comment("Richardson-Lucy iterations"),
        WriteHeaderCard::new("DCAMT", HeaderValue::Float(f64::from(request.amount)))
            .with_comment("restored estimate blend"),
        WriteHeaderCard::new(
            "DCNOISE",
            HeaderValue::Float(f64::from(request.noise_fraction)),
        )
        .with_comment("channel-relative damping floor"),
        WriteHeaderCard::new(
            "DCMAXCOR",
            HeaderValue::Float(f64::from(request.max_correction)),
        )
        .with_comment("per-iteration correction limit"),
    ]
}

pub async fn get_stack_stretch_image(
    ctx: DbContext,
    Path((_db_id, stretch_id)): Path<(String, String)>,
    Query(query): Query<StackPreviewImageQuery>,
) -> Result<Response, AppError> {
    validate_job_id(&stretch_id)?;
    let path = match query.size {
        StackPreviewImageSize::Screen => stretch_preview_path(&ctx.stack_root, &stretch_id),
        StackPreviewImageSize::Original => {
            stretch_original_preview_path(&ctx.stack_root, &stretch_id)
        }
    };
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat stretched PNG: {error}")))?
        .len();
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/png")
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build stretched PNG response: {error}"))
        })
}

pub async fn download_stack_stretch_fits(
    ctx: DbContext,
    Path((_db_id, stretch_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    validate_job_id(&stretch_id)?;
    let manifest = std::fs::read(stretch_manifest_path(&ctx.stack_root, &stretch_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StackStretchPreview>(&bytes).ok())
        .ok_or(AppError::NotFound)?;
    // The furthest-processed linear image: RC-Astro's output already
    // includes any deconvolution that ran before it.
    let (path, label) = if let Some(rc_astro_id) = &manifest.rc_astro_id {
        validate_job_id(rc_astro_id)?;
        (
            super::rc_astro::rc_astro_fits_path(&ctx.stack_root, rc_astro_id),
            "processed",
        )
    } else {
        let deconvolution_id = manifest.deconvolution_id.ok_or(AppError::NotFound)?;
        validate_job_id(&deconvolution_id)?;
        (
            deconvolution_fits_path(&ctx.stack_root, &deconvolution_id),
            "deconvolved",
        )
    };
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| {
            AppError::InternalError(format!("Failed to stat processed FITS: {error}"))
        })?
        .len();
    let filename = format!("psf-guard-{label}-{}.fits", &stretch_id[..12]);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/fits")
        .header(CONTENT_LENGTH, length)
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build processed FITS response: {error}"))
        })
}

/// GET /api/db/{db}/stack-previews/stretch/{id}/stars
pub async fn get_stack_stretch_stars_image(
    ctx: DbContext,
    Path((_db_id, stretch_id)): Path<(String, String)>,
    Query(query): Query<StackPreviewImageQuery>,
) -> Result<Response, AppError> {
    validate_job_id(&stretch_id)?;
    let path = match query.size {
        StackPreviewImageSize::Screen => stretch_stars_preview_path(&ctx.stack_root, &stretch_id),
        StackPreviewImageSize::Original => {
            stretch_stars_original_preview_path(&ctx.stack_root, &stretch_id)
        }
    };
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat stars PNG: {error}")))?
        .len();
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/png")
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build stars PNG response: {error}"))
        })
}

/// GET /api/db/{db}/stack-previews/stretch/{id}/stars-fits
pub async fn download_stack_stretch_stars_fits(
    ctx: DbContext,
    Path((_db_id, stretch_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    validate_job_id(&stretch_id)?;
    let manifest = std::fs::read(stretch_manifest_path(&ctx.stack_root, &stretch_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StackStretchPreview>(&bytes).ok())
        .ok_or(AppError::NotFound)?;
    let rc_astro_id = manifest.rc_astro_id.ok_or(AppError::NotFound)?;
    validate_job_id(&rc_astro_id)?;
    let path = super::rc_astro::rc_astro_stars_path(&ctx.stack_root, &rc_astro_id);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat stars FITS: {error}")))?
        .len();
    let filename = format!("psf-guard-stars-{}.fits", &stretch_id[..12]);
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "application/fits")
        .header(CONTENT_LENGTH, length)
        .header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{filename}\""),
        )
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| {
            AppError::InternalError(format!("Failed to build stars FITS response: {error}"))
        })
}

fn stretch_dir(stack_root: &FsPath, stretch_id: &str) -> PathBuf {
    crate::server::storage::stack_folder(stack_root, crate::server::storage::stack_kind::STRETCH)
        .join(stretch_id)
}

fn stretch_manifest_path(stack_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(stack_root, stretch_id).join("manifest.json")
}

fn stretch_preview_path(stack_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(stack_root, stretch_id).join("preview.png")
}

fn stretch_original_preview_path(stack_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(stack_root, stretch_id).join("preview-original.png")
}

fn stretch_stars_preview_path(stack_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(stack_root, stretch_id).join("stars.png")
}

fn stretch_stars_original_preview_path(stack_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(stack_root, stretch_id).join("stars-original.png")
}

fn deconvolution_dir(stack_root: &FsPath, deconvolution_id: &str) -> PathBuf {
    crate::server::storage::stack_folder(
        stack_root,
        crate::server::storage::stack_kind::DECONVOLUTION,
    )
    .join(deconvolution_id)
}

fn deconvolution_manifest_path(stack_root: &FsPath, deconvolution_id: &str) -> PathBuf {
    deconvolution_dir(stack_root, deconvolution_id).join("manifest.json")
}

fn deconvolution_fits_path(stack_root: &FsPath, deconvolution_id: &str) -> PathBuf {
    deconvolution_dir(stack_root, deconvolution_id).join("deconvolved.fits")
}

fn stretch_artifacts_exist(
    stack_root: &FsPath,
    stretch_id: &str,
    manifest: &StackStretchPreview,
) -> bool {
    stretch_preview_path(stack_root, stretch_id).is_file()
        && stretch_original_preview_path(stack_root, stretch_id).is_file()
        && manifest.deconvolution_id.as_ref().is_none_or(|id| {
            validate_job_id(id).is_ok() && deconvolution_fits_path(stack_root, id).is_file()
        })
        && manifest.rc_astro_id.as_ref().is_none_or(|id| {
            validate_job_id(id).is_ok()
                && super::rc_astro::rc_astro_fits_path(stack_root, id).is_file()
                && (manifest.stars_fits_url.is_none()
                    || super::rc_astro::rc_astro_stars_path(stack_root, id).is_file())
        })
        && (manifest.stars_preview_url.is_none()
            || stretch_stars_preview_path(stack_root, stretch_id).is_file())
}

pub(super) fn write_json_atomic(path: &FsPath, value: &impl Serialize) -> Result<(), String> {
    // Windows cannot replace a just-persisted file while another writer still
    // holds its returned handle. Keep publication and handle close together.
    static PUBLICATION_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _guard = PUBLICATION_LOCK.lock().map_err(|error| error.to_string())?;
    let parent = path
        .parent()
        .ok_or_else(|| "Stretch manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|error| error.to_string())?;
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    temporary
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// What a 202 poll answer carries: where the run stands. While it waits for
/// the stack worker, `queued` is true, `stage` says what it waits for and
/// there is no `fraction`; once it runs, `stage` and `fraction` are its live
/// progress. Clients that read only `stage` and `fraction` keep working.
#[derive(Debug, Serialize)]
pub(super) struct StretchPendingStatus {
    pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fraction: Option<f32>,
    /// Waiting in the line for the stack worker.
    queued: bool,
    /// What holds the worker while the run waits, such as
    /// `the stack build of M44 · Ha`.
    #[serde(skip_serializing_if = "Option::is_none")]
    waiting_for: Option<String>,
    /// Place in the line, 0 next.
    #[serde(skip_serializing_if = "Option::is_none")]
    queue_position: Option<usize>,
}

/// Where a pending run stands now: waiting in the line, or running with its
/// live progress.
pub(super) fn pending_status(
    stretch_id: &str,
    manager: &super::StackPreviewManager,
) -> StretchPendingStatus {
    if let Some(wait) = manager.processing_wait(stretch_id) {
        return StretchPendingStatus {
            pending: true,
            stage: Some(wait.describe()),
            fraction: None,
            queued: true,
            waiting_for: wait.holder,
            queue_position: wait.position,
        };
    }
    let progress = run_progress(stretch_id);
    StretchPendingStatus {
        pending: true,
        stage: progress.as_ref().map(|progress| progress.stage.clone()),
        fraction: progress.and_then(|progress| progress.fraction),
        queued: false,
        waiting_for: None,
        queue_position: None,
    }
}

/// A ready result answers 200 with the preview; a pending one answers 202
/// with where the run stands, telling the client to re-send the same
/// request.
pub(super) fn apply_response(
    outcome: StretchApplyOutcome,
    manager: &super::StackPreviewManager,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match outcome {
        StretchApplyOutcome::Ready(result) => Json(ApiResponse::success(*result)).into_response(),
        StretchApplyOutcome::Pending { stretch_id } => (
            StatusCode::ACCEPTED,
            Json(ApiResponse::success(pending_status(&stretch_id, manager))),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn persisted_processing_fixture(cache: &FsPath, stretch_id: &str) -> StackStretchPreview {
        let request: StackViewProcessingRequest = serde_json::from_value(serde_json::json!({
            "model": {"type": "identity"}, "rc_astro": {"steps": [{"tool": "bxt", "parameters": {"sharpen": 0.4}}]}
        })).unwrap();
        let analysis = StretchAnalysis::analyze(&[0.1, 0.2, 0.3, 0.4], 1, 4).unwrap();
        let identity = StretchIdentity {
            stretch_id: stretch_id.into(),
            deconvolution_id: None,
            rc_astro_chain: None,
            selection_path: PathBuf::new(),
            request,
        };
        let preview = build_stretch_preview(
            "db",
            &identity,
            false,
            RenderedVariant {
                config: identity.request.stretch.config(),
                resolved_plan: serde_json::json!({}),
                source_transfer: StackStretchSourceTransfer::Linear,
                input_range: None,
                linked_statistics: analysis.linked_statistics(),
                channel_statistics: analysis.channel_statistics(),
                luminance_statistics: analysis.luminance_statistics(),
                deconvolution: None,
                rc_astro: Some(super::super::rc_astro::StackRcAstroResult {
                    cli_version: "2.6.6".into(),
                    steps: vec![],
                    has_stars: false,
                }),
                color_calibration: None,
                color_calibration_basis: None,
            },
        );
        write_json_atomic(&stretch_manifest_path(cache, stretch_id), &preview).unwrap();
        std::fs::write(stretch_preview_path(cache, stretch_id), b"preview").unwrap();
        std::fs::write(
            stretch_original_preview_path(cache, stretch_id),
            b"original",
        )
        .unwrap();
        preview
    }

    /// The median anchor stretch from Seiza arrives as the browser sends it
    /// and lands the background median on its target.
    #[test]
    fn a_median_anchor_request_from_the_browser_stretches_the_median_to_its_target() {
        let request: StackViewProcessingRequest = serde_json::from_value(serde_json::json!({
            "model": {
                "type": "median-anchor",
                "target_median": 0.3,
                "black_point_sigma": 2.7,
                "curves_boost": 0,
                "hdr_amount": 0,
                "hdr_knee": 0.75,
                "normalize": false
            },
            "color_strategy": "linked"
        }))
        .unwrap();
        let data: Vec<f32> = (0..4_001)
            .map(|index| 0.05 + (index % 97) as f32 * 0.0003)
            .collect();
        let config = request.stretch.config();
        let plan = config.resolve_for(&data, 1).unwrap();
        let median = StretchAnalysis::analyze(&data, 1, data.len())
            .unwrap()
            .linked_statistics()
            .median as f32;
        assert!((plan.curves()[0].map(median) - 0.3).abs() < 1.0e-4);
        // Without black clipping the black point is the minimum.
        let unclipped: StackViewProcessingRequest = serde_json::from_value(serde_json::json!({
            "model": {"type": "median-anchor", "target_median": 0.25, "black_point_sigma": null}
        }))
        .unwrap();
        let plan = unclipped.stretch.config().resolve_for(&data, 1).unwrap();
        assert_eq!(plan.curves()[0].map(0.05), 0.0);
    }

    /// The layered stretch blurs the whole image, so it only works when the
    /// render passes the image's shape. Mono and RGB stacks both reach it.
    #[test]
    fn a_layered_request_from_the_browser_renders_mono_and_rgb_stacks() {
        let request: StackViewProcessingRequest = serde_json::from_value(serde_json::json!({
            "model": {
                "type": "layered",
                "target_background": 0.2,
                "aggressiveness": 0.5,
                "dynamic_range_compression": 0.5,
                "scale_separation": 16,
                "contrast_recovery": 0.6,
                "saturation_amount": 0.3,
                "saturation_boost": 0.5,
                "lightness_mask": true
            },
            "color_strategy": "linked"
        }))
        .unwrap();
        let config = request.stretch.config();
        let (width, height) = (64, 48);
        // A noisy background near 0.05 with a bright square in one corner.
        let level = |index: usize| {
            let (x, y) = (index % width, index / width);
            let noise = ((index * 7919) % 101) as f32 * 0.0001;
            if x < 12 && y < 12 {
                0.6 + noise
            } else {
                0.05 + noise
            }
        };
        let median_of = |mut values: Vec<u8>| {
            values.sort_unstable();
            f32::from(values[values.len() / 2]) / 255.0
        };

        let mono =
            LinearImage::new(width, height, 1, (0..width * height).map(level).collect()).unwrap();
        let (rendered, _) = render_dynamic_image(&mono, &config).unwrap();
        let median = median_of(rendered.into_luma8().into_raw());
        assert!((median - 0.2).abs() < 0.03, "mono median {median}");

        let rgb = LinearImage::new(
            width,
            height,
            3,
            (0..width * height)
                .flat_map(|index| {
                    let value = level(index);
                    [value, value * 0.9, value * 0.8]
                })
                .collect(),
        )
        .unwrap();
        let (rendered, _) = render_dynamic_image(&rgb, &config).unwrap();
        let rendered = rendered.into_rgb8();
        let red = median_of(rendered.pixels().map(|pixel| pixel[0]).collect());
        let blue = median_of(rendered.pixels().map(|pixel| pixel[2]).collect());
        assert!(
            red > blue,
            "colour follows the input: red {red}, blue {blue}"
        );
    }

    #[test]
    fn selected_processing_restores_request_and_versions_but_not_another_revision() {
        let cache = tempfile::tempdir().unwrap();
        let id = "a".repeat(64);
        let expected = persisted_processing_fixture(cache.path(), &id);
        let path = selection_path(cache.path(), "mono:job:0", "revision1");
        request_selection(&path, &id).unwrap();
        bind_selection(&path, &id, &id).unwrap();
        complete_selection(&path, &id).unwrap();
        let restored = selected_processing(cache.path(), "mono:job:0", "revision1")
            .unwrap()
            .unwrap();
        assert_eq!(
            serde_json::to_value(restored.request).unwrap(),
            serde_json::to_value(expected.request).unwrap()
        );
        assert_eq!(restored.rc_astro.unwrap().cli_version, "2.6.6");
        assert!(selected_processing(cache.path(), "mono:job:0", "revision2")
            .unwrap()
            .is_none());
        assert!(selected_processing(cache.path(), "mono:job:1", "revision1")
            .unwrap()
            .is_none());
    }

    #[test]
    fn processing_selection_preserves_previous_until_success_and_revert_wins() {
        let cache = tempfile::tempdir().unwrap();
        let first = "a".repeat(64);
        let second = "b".repeat(64);
        persisted_processing_fixture(cache.path(), &first);
        persisted_processing_fixture(cache.path(), &second);
        let path = selection_path(cache.path(), "mono:job:0", "rev");
        request_selection(&path, &first).unwrap();
        bind_selection(&path, &first, &first).unwrap();
        complete_selection(&path, &first).unwrap();
        request_selection(&path, &second).unwrap();
        bind_selection(&path, &second, &second).unwrap();
        assert_eq!(
            selected_processing(cache.path(), "mono:job:0", "rev")
                .unwrap()
                .unwrap()
                .stretch_id,
            first
        );
        complete_selection(&path, &second).unwrap();
        complete_selection(&path, &first).unwrap();
        assert_eq!(
            selected_processing(cache.path(), "mono:job:0", "rev")
                .unwrap()
                .unwrap()
                .stretch_id,
            second
        );
        clear_selected_processing(cache.path(), "mono:job:0", "rev").unwrap();
        complete_selection(&path, &second).unwrap();
        assert!(selected_processing(cache.path(), "mono:job:0", "rev")
            .unwrap()
            .is_none());
        assert!(stretch_manifest_path(cache.path(), &second).is_file());
    }

    #[test]
    fn late_schema_probe_cannot_replace_newer_intent_or_reenable_reverted_processing() {
        let cache = tempfile::tempdir().unwrap();
        let path = selection_path(cache.path(), "mono:job:0", "rev");
        request_selection(&path, "old-probe").unwrap();
        request_selection(&path, "new-probe").unwrap();
        bind_selection(&path, "new-probe", "new-result").unwrap();
        bind_selection(&path, "old-probe", "old-result").unwrap();
        complete_selection(&path, "old-result").unwrap();
        assert!(read_selection(&path).unwrap().selected.is_none());
        complete_selection(&path, "new-result").unwrap();
        assert_eq!(
            read_selection(&path).unwrap().selected.as_deref(),
            Some("new-result")
        );
        clear_selected_processing(cache.path(), "mono:job:0", "rev").unwrap();
        bind_selection(&path, "old-probe", "old-result").unwrap();
        complete_selection(&path, "old-result").unwrap();
        assert!(read_selection(&path).is_none());
    }

    #[test]
    fn durable_rc_astro_selection_is_pinned_only_while_source_revision_exists() {
        let cache = tempfile::tempdir().unwrap();
        let job_id = "a".repeat(64);
        let stretch_id = "b".repeat(64);
        let rc_id = "c".repeat(64);
        let key = format!("mono:{job_id}:0");
        let mut preview = persisted_processing_fixture(cache.path(), &stretch_id);
        preview.rc_astro_id = Some(rc_id.clone());
        write_json_atomic(&stretch_manifest_path(cache.path(), &stretch_id), &preview).unwrap();
        let fits = super::super::rc_astro::rc_astro_fits_path(cache.path(), &rc_id);
        std::fs::create_dir_all(fits.parent().unwrap()).unwrap();
        std::fs::write(&fits, b"rc-fits").unwrap();
        let source_manifest = super::super::manifest_path(cache.path(), &job_id);
        write_json_atomic(&source_manifest, &serde_json::json!({"artifact_revision": "rev1", "groups": [{"index":0,"state":"ready"}]})).unwrap();
        std::fs::write(super::super::fits_path(cache.path(), &job_id, 0), b"source").unwrap();
        let path = selection_path(cache.path(), &key, "rev1");
        request_selection_with_source(
            &path,
            "generation",
            Some(ProcessingSource {
                key,
                revision: "rev1".into(),
            }),
            false,
        )
        .unwrap();
        bind_selection(&path, "generation", &stretch_id).unwrap();
        complete_selection(&path, &stretch_id).unwrap();
        let no_active_sources = std::collections::HashMap::new();
        assert!(selected_rc_astro_ids(cache.path(), &no_active_sources).contains(&rc_id));
        let old = filetime::FileTime::from_system_time(
            std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600),
        );
        filetime::set_file_mtime(&path, old).unwrap();
        assert!(
            selected_rc_astro_ids(cache.path(), &no_active_sources).contains(&rc_id),
            "a durable matching source stays pinned beyond the grace period"
        );
        filetime::set_file_mtime(&path, filetime::FileTime::now()).unwrap();
        write_json_atomic(&source_manifest, &serde_json::json!({"artifact_revision": "rev2", "groups": [{"index":0,"state":"ready"}]})).unwrap();
        assert!(
            selected_rc_astro_ids(cache.path(), &no_active_sources).contains(&rc_id),
            "an in-memory ready group can precede the whole-job manifest"
        );
        filetime::set_file_mtime(
            &path,
            filetime::FileTime::from_system_time(
                std::time::SystemTime::now() - std::time::Duration::from_secs(48 * 3600),
            ),
        )
        .unwrap();
        let active_sources = std::collections::HashMap::from([(job_id, "rev1".into())]);
        assert!(
            selected_rc_astro_ids(cache.path(), &active_sources).contains(&rc_id),
            "in-memory ready sources remain pinned even before whole-job publication"
        );
        assert!(selected_rc_astro_ids(cache.path(), &no_active_sources).is_empty());
        assert!(!path.exists());
    }

    #[test]
    fn concurrent_manifest_writers_publish_complete_documents() {
        let cache = tempfile::tempdir().unwrap();
        let path = cache.path().join("manifest.json");
        std::thread::scope(|scope| {
            for writer in 0..8 {
                let path = &path;
                scope.spawn(move || {
                    for revision in 0..8 {
                        write_json_atomic(path, &serde_json::json!({"writer":writer, "revision":revision, "payload":"x".repeat(4000)})).unwrap();
                    }
                });
            }
        });
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["payload"].as_str().unwrap().len(), 4000);
    }

    #[test]
    fn parameterized_stretch_renders_screen_and_original_without_touching_source() {
        let cache = tempfile::tempdir().unwrap();
        let image = LinearImage::new(
            64,
            48,
            1,
            (0..64 * 48).map(|index| index as f32 + 100.0).collect(),
        )
        .unwrap();
        let source = image.data.clone();
        let output = cache.path().join("nested").join("variant");
        let screen = output.join("screen.png");
        let original = output.join("original.png");

        let plan = render_image_previews_atomic(
            &image,
            &StretchConfig::percentile_asinh(0.02, 0.99, 6.0, 2_000),
            StackStretchSourceTransfer::Linear,
            &screen,
            &original,
        )
        .unwrap();

        assert!(screen.is_file());
        assert!(original.is_file());
        assert_eq!(image.data, source);
        assert_eq!(plan["curves"][0]["type"], "asinh");
    }

    #[test]
    fn single_preview_render_writes_only_the_requested_crop() {
        let cache = tempfile::tempdir().unwrap();
        let image = LinearImage::new(
            16,
            12,
            1,
            (0..16 * 12).map(|index| index as f32 + 1.0).collect(),
        )
        .unwrap();
        let destination = cache.path().join("crop").join("source.png");

        let plan = render_image_preview_atomic(
            &image,
            &default_linear_config(),
            StackStretchSourceTransfer::Linear,
            &destination,
        )
        .unwrap();

        assert!(destination.is_file());
        assert_eq!(
            std::fs::read_dir(destination.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        assert_eq!(plan["curves"][0]["type"], "mtf");
    }

    #[test]
    fn browser_request_shapes_match_seiza_models() {
        let requests = [
            serde_json::json!({"model": {"type": "identity"}, "color_strategy": "linked"}),
            serde_json::json!({
                "model": {"type": "linear", "black": 10.0, "white": 20.0},
                "color_strategy": "linked"
            }),
            serde_json::json!({
                "model": {"type": "asinh", "black": 10.0, "white": 20.0, "strength": 8.0},
                "color_strategy": "linked"
            }),
            serde_json::json!({
                "model": {
                    "type": "percentile-asinh",
                    "black_percentile": 0.01,
                    "white_percentile": 0.995,
                    "strength": 8.0
                },
                "color_strategy": "unlinked"
            }),
            serde_json::json!({
                "model": {
                    "type": "mtf", "shadows": 10.0, "midtone": 0.2, "highlights": 20.0
                },
                "color_strategy": "linked"
            }),
            serde_json::json!({
                "model": {
                    "type": "ghs",
                    "stretch_factor": 1.0,
                    "local_intensity": 0.0,
                    "symmetry_point": 0.4,
                    "protect_shadows": 0.1,
                    "protect_highlights": 0.9,
                    "black": 10.0,
                    "white": 20.0
                },
                "color_strategy": "luminance-preserving"
            }),
            serde_json::json!({
                "model": {"type": "auto-mtf", "target_median": 0.2, "shadows_clip": -2.8},
                "color_strategy": "linked"
            }),
        ];

        for request in requests {
            let parsed = serde_json::from_value::<StackStretchRequest>(request).unwrap();
            assert!(serde_json::to_value(&parsed)
                .unwrap()
                .get("deconvolution")
                .is_none());
        }

        let request = serde_json::json!({
            "model": {"type": "auto-mtf", "target_median": 0.2, "shadows_clip": -2.8},
            "color_strategy": "linked",
            "deconvolution": {
                "psf_fwhm_pixels": 3.1,
                "iterations": 4,
                "amount": 0.35,
                "noise_fraction": 0.001,
                "max_correction": 2.0
            }
        });
        let parsed = serde_json::from_value::<StackViewProcessingRequest>(request).unwrap();
        assert_eq!(
            parsed.deconvolution,
            Some(DeconvolutionConfig::conservative(3.1))
        );
    }

    #[test]
    fn opt_in_deconvolution_sharpens_a_linear_star_without_mutating_source() {
        let size = 31;
        let center = size / 2;
        let sigma = 3.1_f32 / 2.354_82;
        let pixels = (0..size * size)
            .map(|index| {
                let x = index % size;
                let y = index / size;
                let radius_squared = ((x as isize - center as isize).pow(2)
                    + (y as isize - center as isize).pow(2))
                    as f32;
                (-0.5 * radius_squared / sigma.powi(2)).exp()
            })
            .collect::<Vec<_>>();
        let image = LinearImage::new(size, size, 1, pixels.clone()).unwrap();

        let (restored, result) =
            apply_deconvolution(&image, DeconvolutionConfig::conservative(3.1)).unwrap();

        assert_eq!(image.data, pixels);
        assert!(restored.data[center * size + center] > image.data[center * size + center]);
        assert_eq!(result.channels.len(), 1);
        assert!(result.channels[0].output_peak > result.channels[0].input_peak);
    }

    #[test]
    fn deconvolution_uses_upstream_parameter_validation() {
        let invalid = DeconvolutionConfig {
            iterations: 0,
            ..DeconvolutionConfig::conservative(3.1)
        };

        assert!(invalid
            .validate()
            .unwrap_err()
            .to_string()
            .contains("iterations"));
    }

    #[test]
    fn deconvolution_cache_identity_is_independent_of_display_stretch() {
        let config = DeconvolutionConfig::conservative(3.1);
        let first = deconvolution_cache_id("db", "mono:job:0", "revision", &config).unwrap();
        let second = deconvolution_cache_id("db", "mono:job:0", "revision", &config).unwrap();
        let changed = deconvolution_cache_id(
            "db",
            "mono:job:0",
            "revision",
            &DeconvolutionConfig {
                amount: 0.5,
                ..config
            },
        )
        .unwrap();

        assert_eq!(first, second);
        assert_ne!(first, changed);
    }

    #[test]
    fn masked_deconvolution_keeps_registered_borders() {
        let size = 31;
        let mut pixels = vec![0.1; size * size];
        pixels[size / 2 * size + size / 2] = 5.0;
        for row in pixels.chunks_exact_mut(size) {
            row[0] = f32::NAN;
        }
        let image = LinearImage::new(size, size, 1, pixels).unwrap();

        let (restored, _) =
            apply_deconvolution(&image, DeconvolutionConfig::conservative(3.1)).unwrap();

        assert!(restored.data.chunks_exact(size).all(|row| row[0].is_nan()));
        assert!(restored
            .data
            .chunks_exact(size)
            .all(|row| row[1..].iter().all(|sample| sample.is_finite())));
    }

    #[test]
    fn linear_normalization_samples_every_rgb_channel() {
        let image = LinearImage::new(
            128,
            96,
            3,
            (0..128 * 96)
                .flat_map(|_| [10.0_f32, 100.0_f32, 1_000.0_f32])
                .collect(),
        )
        .unwrap();

        let (normalized, range) = normalize_linear_image(&image).unwrap();

        assert_eq!(range.black, 10.0);
        assert_eq!(range.white, 1_000.0);
        assert_eq!(&normalized.data[..3], &[0.0, 90.0 / 990.0, 1.0]);
    }

    /// A one-shot-color stack's processing is calibrated by default, and its
    /// id follows the calibration settings, the solve and the catalog; a mono
    /// stack's id does not change.
    #[tokio::test]
    async fn a_color_stack_run_keys_on_its_calibration() {
        use super::super::color_calibration::{
            tests::solution, StackCalibrationSource, StackColorCalibration,
        };
        let request: StackViewProcessingRequest =
            serde_json::from_value(serde_json::json!({"model": {"type": "identity"}})).unwrap();
        let catalog = crate::astrometry::AstrometryCatalogFileSignature {
            name: "gaia_photometry".into(),
            path: "/catalogs/stars-gaia-photometry.bin".into(),
            format: "SEIZAPH1".into(),
            size_bytes: 10,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
            sha256: None,
        };
        let source =
            |solved: bool, catalog: Option<crate::astrometry::AstrometryCatalogFileSignature>| {
                StackCalibrationSource {
                    reference_image_id: Some(7),
                    orientation: Some(super::super::StackSkyOrientation {
                        convention: seiza_stacking::SKY_ORIENTATION_NAME.into(),
                        version: seiza_stacking::SKY_ORIENTATION_VERSION,
                        source: "test".into(),
                        output_width: 1000,
                        output_height: 800,
                        source_to_output: seiza_stacking::AffineTransform::IDENTITY,
                    }),
                    solution: solved.then(solution),
                    catalog,
                }
            };
        async fn id(
            calibration: Option<&StackCalibrationSource>,
            request: StackViewProcessingRequest,
        ) -> PreparedRun {
            prepare_run("db", "mono:job:0", "rev", calibration, request)
                .await
                .unwrap()
        }
        let mono = id(None, request.clone()).await;
        assert!(mono.color_calibration.is_none());
        assert!(mono.request().color_calibration.is_none());

        let ready = source(true, Some(catalog.clone()));
        let color = id(Some(&ready), request.clone()).await;
        assert_eq!(
            color.request().color_calibration,
            Some(StackColorCalibration::default())
        );
        assert!(color.color_calibration.as_ref().unwrap().placement.is_ok());
        assert_ne!(color.stretch_id, mono.stretch_id);

        // A newly installed catalog, or a new solve, is a new run.
        let unsolved = source(false, Some(catalog.clone()));
        assert_ne!(
            id(Some(&unsolved), request.clone()).await.stretch_id,
            color.stretch_id
        );
        let no_catalog = source(true, None);
        assert_ne!(
            id(Some(&no_catalog), request.clone()).await.stretch_id,
            color.stretch_id
        );

        // Turned off, the stack keeps the setting but plans nothing.
        let mut off = request;
        off.color_calibration = Some(StackColorCalibration {
            enabled: false,
            ..StackColorCalibration::default()
        });
        let off = id(Some(&ready), off).await;
        assert!(off.color_calibration.is_none());
        assert_ne!(off.stretch_id, color.stretch_id);
    }
}
