//! Reversible display-stretch variants rendered from cached linear stack FITS.

use super::{
    save_png_atomic, validate_job_id, StackPreviewImageQuery, StackPreviewImageSize,
    PREVIEW_MAX_DIMENSION,
};
use axum::{
    body::Body,
    extract::{Path, Query},
    http::{
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::Response,
    Json,
};
use rayon::ThreadPoolBuilder;
use seiza_stacking::{FitsFrame, LinearImage};
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
const STRETCH_ANALYSIS_SAMPLES: usize = 200_000;
const STRETCH_BYTES_PER_SAMPLE: u64 = 16;
const LINEAR_BLACK_PERCENTILE: f64 = 0.001;
const LINEAR_WHITE_PERCENTILE: f64 = 0.999;
pub(super) const SEIZA_STRETCH_VERSION: &str = "0.1.0-git-d6b8dfc";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StackStretchRequest {
    pub model: StretchModel,
    #[serde(default)]
    pub color_strategy: ColorStrategy,
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
    pub config: StretchConfig,
    pub resolved_plan: serde_json::Value,
    pub source_transfer: StackStretchSourceTransfer,
    pub input_range: Option<StackStretchInputRange>,
    pub linked_statistics: RobustStatistics,
    pub channel_statistics: Vec<Option<RobustStatistics>>,
    pub luminance_statistics: Option<RobustStatistics>,
    pub preview_url: String,
    pub original_preview_url: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub struct StackStretchInputRange {
    pub black: f32,
    pub white: f32,
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
    request: StackStretchRequest,
) -> Result<StackStretchPreview, AppError> {
    let config = request.config();
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
    let mut stretch_id = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut stretch_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    let manifest_path = stretch_manifest_path(&cache_root, &stretch_id);
    if let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(cached) = serde_json::from_slice::<StackStretchPreview>(&bytes)
        && stretch_artifacts_exist(&cache_root, &stretch_id)
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
        && stretch_artifacts_exist(&cache_root, &stretch_id)
    {
        return Ok(cached);
    }
    let guard = state.begin_interactive_job();
    let state_for_render = Arc::clone(&state);
    let cache_for_render = cache_root.clone();
    let id_for_render = stretch_id.clone();
    let rendered = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        render_fits_variant(
            &state_for_render,
            &cache_for_render,
            &id_for_render,
            &source_path,
            &config,
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Stretch worker failed: {error}")))?
    .map_err(AppError::BadRequest)?;

    let response = StackStretchPreview {
        schema_version: 1,
        stretch_id: stretch_id.clone(),
        stretch_version: SEIZA_STRETCH_VERSION.into(),
        config: rendered.config,
        resolved_plan: rendered.resolved_plan,
        source_transfer: rendered.source_transfer,
        input_range: rendered.input_range,
        linked_statistics: rendered.linked_statistics,
        channel_statistics: rendered.channel_statistics,
        luminance_statistics: rendered.luminance_statistics,
        preview_url: format!("/api/db/{database_id}/stack-previews/stretch/{stretch_id}/preview"),
        original_preview_url: format!(
            "/api/db/{database_id}/stack-previews/stretch/{stretch_id}/preview?size=original"
        ),
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
}

fn render_fits_variant(
    state: &Arc<AppState>,
    cache_root: &FsPath,
    stretch_id: &str,
    source_path: &FsPath,
    config: &StretchConfig,
) -> Result<RenderedVariant, String> {
    let frame = FitsFrame::open(source_path).map_err(|error| error.to_string())?;
    let samples = frame.image.data.len();
    let estimate = (samples as u64).saturating_mul(STRETCH_BYTES_PER_SAMPLE);
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
    let normalized = if source_transfer == StackStretchSourceTransfer::Linear {
        Some(normalize_linear_image(&frame.image)?)
    } else {
        None
    };
    let input_range = normalized.as_ref().map(|(_, range)| *range);
    let prepared = normalized
        .as_ref()
        .map(|(image, _)| image)
        .unwrap_or(&frame.image);
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
    })
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

fn stretch_artifacts_exist(cache_root: &FsPath, stretch_id: &str) -> bool {
    stretch_preview_path(cache_root, stretch_id).is_file()
        && stretch_original_preview_path(cache_root, stretch_id).is_file()
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
            serde_json::from_value::<StackStretchRequest>(request).unwrap();
        }
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
