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
use rayon::ThreadPoolBuilder;
use seiza_deconvolution::{
    deconvolve_masked, ChannelDiagnostics, DeconvolutionConfig, ALGORITHM_VERSION,
};
use seiza_fits::{HeaderValue, WriteHeaderCard};
use seiza_stacking::{write_processed_image_fits_f32, FitsFrame, LinearImage};
use seiza_stretch::{
    ColorStrategy, RobustStatistics, StretchAnalysis, StretchConfig, StretchModel, StretchParams,
    StretchPlan,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
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
pub(super) const SEIZA_STRETCH_VERSION: &str = "0.1.0-git-b9bdcd1";

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
    pub resolved_plan: serde_json::Value,
    pub source_transfer: StackStretchSourceTransfer,
    pub input_range: Option<StackStretchInputRange>,
    pub linked_statistics: RobustStatistics,
    pub channel_statistics: Vec<Option<RobustStatistics>>,
    pub luminance_statistics: Option<RobustStatistics>,
    #[serde(default)]
    pub deconvolution: Option<StackDeconvolutionResult>,
    pub preview_url: String,
    pub original_preview_url: String,
    #[serde(default)]
    pub fits_url: Option<String>,
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

pub(super) fn default_linear_config() -> StretchConfig {
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

struct StretchRender {
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

pub(super) fn normalize_linear_image(
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
        .apply_u8(&image.data, image.channels)
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
    progress(StackPreviewRenderPhase::Original);
    save_png_atomic(&dynamic, original_destination)?;
    progress(StackPreviewRenderPhase::Screen);
    let resized = dynamic.resize(
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_DIMENSION,
        image::imageops::FilterType::Lanczos3,
    );
    save_png_atomic(&resized, screen_destination)?;
    Ok(StretchRender { plan })
}

pub(super) async fn apply_to_fits(
    state: Arc<AppState>,
    database_id: String,
    cache_root: PathBuf,
    source_key: String,
    source_revision: String,
    source_path: PathBuf,
    request: StackViewProcessingRequest,
) -> Result<StackStretchPreview, AppError> {
    if let Some(deconvolution) = request.deconvolution {
        deconvolution
            .validate()
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
    }
    let config = request.stretch.config();
    let deconvolution = request.deconvolution;
    let deconvolution_id = deconvolution
        .map(|request| {
            deconvolution_cache_id(&database_id, &source_key, &source_revision, &request)
        })
        .transpose()?;
    let encoded = serde_json::to_vec(&config).map_err(|error| {
        AppError::InternalError(format!("Failed to encode stretch request: {error}"))
    })?;
    let mut hasher = Sha256::new();
    hasher.update(database_id.as_bytes());
    hasher.update(source_key.as_bytes());
    hasher.update(source_revision.as_bytes());
    hasher.update(STRETCH_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STRETCH_VERSION.as_bytes());
    hasher.update(&encoded);
    if let Some(deconvolution_id) = &deconvolution_id {
        hasher.update(b"deconvolution");
        hasher.update(deconvolution_id.as_bytes());
    }
    let mut stretch_id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut stretch_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    let manifest_path = stretch_manifest_path(&cache_root, &stretch_id);
    if let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(cached) = serde_json::from_slice::<StackStretchPreview>(&bytes)
        && stretch_artifacts_exist(&cache_root, &stretch_id, &cached)
    {
        return Ok(cached);
    }

    let permit = Arc::clone(&state.stack_previews.permit);
    let _permit = permit
        .acquire_owned()
        .await
        .map_err(|_| AppError::InternalError("Stack preview processor is unavailable".into()))?;
    if let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(cached) = serde_json::from_slice::<StackStretchPreview>(&bytes)
        && stretch_artifacts_exist(&cache_root, &stretch_id, &cached)
    {
        return Ok(cached);
    }
    let guard = state.begin_interactive_job();
    let state_for_render = Arc::clone(&state);
    let cache_for_render = cache_root.clone();
    let id_for_render = stretch_id.clone();
    let deconvolution_id_for_render = deconvolution_id.clone();
    let rendered = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        render_fits_variant(
            &state_for_render,
            &cache_for_render,
            &id_for_render,
            &source_path,
            &config,
            deconvolution,
            deconvolution_id_for_render.as_deref(),
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Stretch worker failed: {error}")))?
    .map_err(AppError::BadRequest)?;

    let response = StackStretchPreview {
        schema_version: 2,
        stretch_id: stretch_id.clone(),
        stretch_version: SEIZA_STRETCH_VERSION.into(),
        deconvolution_version: rendered
            .deconvolution
            .as_ref()
            .map(|_| deconvolution_version()),
        deconvolution_id: deconvolution_id.clone(),
        config: rendered.config,
        resolved_plan: rendered.resolved_plan,
        source_transfer: rendered.source_transfer,
        input_range: rendered.input_range,
        linked_statistics: rendered.linked_statistics,
        channel_statistics: rendered.channel_statistics,
        luminance_statistics: rendered.luminance_statistics,
        deconvolution: rendered.deconvolution,
        preview_url: format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/preview"),
        original_preview_url: format!(
            "/api/db/{database_id}/stack-previews/stretch/{stretch_id}/preview?size=original"
        ),
        fits_url: deconvolution
            .map(|_| format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/fits")),
    };
    write_json_atomic(&manifest_path, &response).map_err(AppError::InternalError)?;
    Ok(response)
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
}

fn render_fits_variant(
    state: &Arc<AppState>,
    cache_root: &FsPath,
    stretch_id: &str,
    source_path: &FsPath,
    config: &StretchConfig,
    deconvolution_request: Option<DeconvolutionConfig>,
    deconvolution_id: Option<&str>,
) -> Result<RenderedVariant, String> {
    let frame = FitsFrame::open(source_path).map_err(|error| error.to_string())?;
    let samples = frame.image.data.len();
    let bytes_per_sample = if deconvolution_request.is_some() {
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
    let budget = crate::concurrency::plan_workers(
        None,
        &policy,
        crate::concurrency::Priority::Interactive,
        Some(frame.image.pixel_count()),
    );
    let pool = ThreadPoolBuilder::new()
        .num_threads(budget.workers)
        .thread_name(|index| format!("stack-stretch-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Stack stretch {stretch_id}: {} worker(s) — {}",
        budget.workers,
        budget.rationale
    );
    let source_transfer = frame
        .headers
        .iter()
        .find(|(keyword, _)| keyword.eq_ignore_ascii_case("SEIZATRF"))
        .and_then(|(_, value)| value.as_str())
        .filter(|value| value.eq_ignore_ascii_case("DISPLAY"))
        .map(|_| StackStretchSourceTransfer::DisplayReferred)
        .unwrap_or(StackStretchSourceTransfer::Linear);
    let source_analysis = StretchAnalysis::analyze(
        &frame.image.data,
        frame.image.channels,
        config.max_analysis_samples,
    )
    .map_err(|error| error.to_string())?;
    if deconvolution_request.is_some()
        && source_transfer == StackStretchSourceTransfer::DisplayReferred
    {
        return Err("Deconvolution requires a linear-light stack source".into());
    }
    let mut processed = None;
    let mut cached_processed = None;
    let deconvolution = if let Some(request) = deconvolution_request {
        let deconvolution_id = deconvolution_id
            .ok_or_else(|| "Deconvolution cache identity is missing".to_string())?;
        let fits = deconvolution_fits_path(cache_root, deconvolution_id);
        let manifest = deconvolution_manifest_path(cache_root, deconvolution_id);
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
            cached_processed = Some(FitsFrame::open(&fits).map_err(|error| error.to_string())?);
            Some(StackDeconvolutionResult {
                config: cached.config,
                channels: cached.channels,
            })
        } else {
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
    let screen = stretch_preview_path(cache_root, stretch_id);
    let original = stretch_original_preview_path(cache_root, stretch_id);
    let render = pool.install(|| {
        render_image_previews_with_details(prepared, config, &screen, &original, |_| {})
    })?;
    Ok(RenderedVariant {
        config: config.clone(),
        resolved_plan: serde_json::to_value(render.plan).map_err(|error| error.to_string())?,
        source_transfer,
        input_range,
        linked_statistics: source_analysis.linked_statistics(),
        channel_statistics: source_analysis.channel_statistics(),
        luminance_statistics: source_analysis.luminance_statistics(),
        deconvolution,
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
        StackPreviewImageSize::Screen => stretch_preview_path(&ctx.cache_dir_path, &stretch_id),
        StackPreviewImageSize::Original => {
            stretch_original_preview_path(&ctx.cache_dir_path, &stretch_id)
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
    let manifest = std::fs::read(stretch_manifest_path(&ctx.cache_dir_path, &stretch_id))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<StackStretchPreview>(&bytes).ok())
        .ok_or(AppError::NotFound)?;
    let deconvolution_id = manifest.deconvolution_id.ok_or(AppError::NotFound)?;
    validate_job_id(&deconvolution_id)?;
    let path = deconvolution_fits_path(&ctx.cache_dir_path, &deconvolution_id);
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
    let filename = format!("psf-guard-deconvolved-{}.fits", &stretch_id[..12]);
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

fn stretch_dir(cache_root: &FsPath, stretch_id: &str) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("stretch")
        .join(stretch_id)
}

fn stretch_manifest_path(cache_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(cache_root, stretch_id).join("manifest.json")
}

fn stretch_preview_path(cache_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(cache_root, stretch_id).join("preview.png")
}

fn stretch_original_preview_path(cache_root: &FsPath, stretch_id: &str) -> PathBuf {
    stretch_dir(cache_root, stretch_id).join("preview-original.png")
}

fn deconvolution_dir(cache_root: &FsPath, deconvolution_id: &str) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("deconvolution")
        .join(deconvolution_id)
}

fn deconvolution_manifest_path(cache_root: &FsPath, deconvolution_id: &str) -> PathBuf {
    deconvolution_dir(cache_root, deconvolution_id).join("manifest.json")
}

fn deconvolution_fits_path(cache_root: &FsPath, deconvolution_id: &str) -> PathBuf {
    deconvolution_dir(cache_root, deconvolution_id).join("deconvolved.fits")
}

fn stretch_artifacts_exist(
    cache_root: &FsPath,
    stretch_id: &str,
    manifest: &StackStretchPreview,
) -> bool {
    stretch_preview_path(cache_root, stretch_id).is_file()
        && stretch_original_preview_path(cache_root, stretch_id).is_file()
        && manifest.deconvolution_id.as_ref().is_none_or(|id| {
            validate_job_id(id).is_ok() && deconvolution_fits_path(cache_root, id).is_file()
        })
}

fn write_json_atomic(path: &FsPath, value: &impl Serialize) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Stretch manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

pub(super) fn response(result: StackStretchPreview) -> Json<ApiResponse<StackStretchPreview>> {
    Json(ApiResponse::success(result))
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
