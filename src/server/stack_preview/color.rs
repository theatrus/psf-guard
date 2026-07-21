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
use seiza_stacking::{
    combine_lrgb, combine_narrowband, combine_rgb, resample_to_reference, write_color_fits_f32,
    ColorOptions, ColorTransfer, FitsFrame, ForaxxOptions, LinearImage, NarrowbandPalette,
    Registrar, RegistrationOptions,
};
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

const STACK_COLOR_CACHE_VERSION: u32 = 2;
const MAX_REGISTRATION_RMS_PIXELS: f64 = 2.0;
const COLOR_BYTES_PER_PIXEL: u64 = 64;

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
    fn label(self) -> &'static str {
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackColorSource {
    pub role: StackColorRole,
    pub filter_name: String,
    pub job_id: String,
    pub group_index: usize,
    pub artifact_revision: String,
    pub accepted_frames: usize,
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
    pub created_unix_seconds: i64,
    pub artifact_revision: String,
    pub cache_version: u32,
    pub stacking_version: String,
    pub sources: Vec<StackColorSource>,
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
}

impl StackPreviewManager {
    fn get_color(&self, job_id: &str) -> Option<StackColorJob> {
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

pub async fn get_stack_color_catalog(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<StackColorCatalog>>, AppError> {
    let latest = load_latest_stacks(&ctx, project_id)?;
    let sources = collect_sources(&ctx.cache_dir_path, &latest);
    let targets = availability(&sources);
    let mut jobs = load_latest_colors(&ctx, project_id)?.jobs;
    for job in &mut jobs {
        job.outdated_reason = color_job_outdated_reason(&ctx.cache_dir_path, job, &latest);
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
            state
                .stack_previews
                .persist_latest_color(&prepared.cache_root, &existing)
                .map_err(AppError::InternalError)?;
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

pub async fn apply_stack_color_stretch(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
    Json(request): Json<super::stretch::StackStretchRequest>,
) -> Result<Json<ApiResponse<super::stretch::StackStretchPreview>>, AppError> {
    super::validate_job_id(&job_id)?;
    let job = if let Some(job) = state.stack_previews.get_color(&job_id) {
        job
    } else {
        let bytes = std::fs::read(color_manifest_path(&ctx.cache_dir_path, &job_id))
            .map_err(|_| AppError::NotFound)?;
        serde_json::from_slice::<StackColorJob>(&bytes).map_err(|error| {
            AppError::InternalError(format!("Invalid color preview manifest: {error}"))
        })?
    };
    if job.database_id != ctx.id || job.state != StackJobState::Completed {
        return Err(AppError::NotFound);
    }
    let result = super::stretch::apply_to_fits(
        state,
        ctx.id.clone(),
        ctx.cache_dir_path.clone(),
        format!("color:{job_id}"),
        job.artifact_revision,
        color_fits_path(&ctx.cache_dir_path, &job_id),
        request,
    )
    .await?;
    Ok(super::stretch::response(result))
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
        _ => Ok(()),
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

    let label = composition_label(request.kind, request.palette).to_string();
    let mut hasher = Sha256::new();
    hasher.update(ctx.id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update(request.target_id.to_le_bytes());
    hasher.update(label.as_bytes());
    hasher.update(STACK_COLOR_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_VERSION.as_bytes());
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
            created_unix_seconds: chrono::Utc::now().timestamp(),
            artifact_revision: artifact_revision.clone(),
            cache_version: STACK_COLOR_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            sources,
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
    })
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
        compose_color(state, &prepared.public, &prepared.cache_root)
    }));
    state
        .stack_previews
        .update_color(&job_id, |job| match result {
            Ok(Ok(())) => {
                job.state = StackJobState::Completed;
                job.phase = "Color preview ready".into();
            }
            Ok(Err(error)) => {
                job.state = StackJobState::Failed;
                job.phase = "Color preview failed".into();
                job.error = Some(error);
            }
            Err(_) => {
                job.state = StackJobState::Failed;
                job.phase = "Color worker failed".into();
                job.error = Some("Color worker panicked".into());
            }
        });
    if let Some(job) = state.stack_previews.get_color(&job_id) {
        if let Err(error) = persist_color_manifest(&prepared.cache_root, &job) {
            tracing::warn!("Failed to persist color preview manifest: {error}");
        }
        if job.state == StackJobState::Completed
            && let Err(error) = state
                .stack_previews
                .persist_latest_color(&prepared.cache_root, &job)
        {
            tracing::warn!("Failed to persist latest color preview index: {error}");
        }
    }
}

fn compose_color(
    state: &Arc<AppState>,
    job: &StackColorJob,
    cache_root: &FsPath,
) -> Result<(), String> {
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
    let reference = FitsFrame::open(super::fits_path(
        cache_root,
        &reference_source.job_id,
        reference_source.group_index,
    ))
    .map_err(|error| {
        format!(
            "Failed to read {} channel stack: {error}",
            reference_role.label()
        )
    })?;
    validate_mono(&reference.image, reference_role)?;
    let pixels = reference.image.pixel_count();
    let estimate = (pixels as u64).saturating_mul(COLOR_BYTES_PER_PIXEL);
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

    pool.install(|| {
        let reference_headers = reference.headers;
        let reference_image = reference.image;
        let registrar = Registrar::new(&reference_image, RegistrationOptions::default())
            .map_err(|error| error.to_string())?;
        let mut images = BTreeMap::new();
        images.insert(reference_role, reference_image);
        state.stack_previews.update_color(&job.job_id, |current| {
            current.processed_channels = 1;
            current.phase = format!("Registering {} channels", current.total_channels);
        });

        for source in job
            .sources
            .iter()
            .filter(|source| source.role != reference_role)
        {
            state.stack_previews.update_color(&job.job_id, |current| {
                current.phase = format!("Registering {}", source.role.label());
            });
            let frame = FitsFrame::open(super::fits_path(
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
            images.insert(source.role, aligned);
            state.stack_previews.update_color(&job.job_id, |current| {
                current.processed_channels += 1;
            });
        }

        state.stack_previews.update_color(&job.job_id, |current| {
            current.phase = format!("Composing {}", current.label);
        });
        let options = ColorOptions::default();
        let composition = match job.kind {
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

        let fits_destination = color_fits_path(cache_root, &job.job_id);
        let parent = fits_destination
            .parent()
            .ok_or_else(|| "Color FITS path has no parent".to_string())?;
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        let temporary = fits_destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        write_color_fits_f32(&temporary, &composition, &reference_headers, &job.label)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &fits_destination).map_err(|error| error.to_string())?;
        state.stack_previews.update_color(&job.job_id, |current| {
            current.phase = "Rendering color preview".into();
        });
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
        super::stretch::render_image_previews_atomic(
            &composition.image,
            &stretch_config,
            source_transfer,
            &color_preview_path(cache_root, &job.job_id),
            &color_original_preview_path(cache_root, &job.job_id),
        )
        .map(|_| ())
    })
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
            Ok(latest)
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
            Ok(latest)
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
    cache_root: &FsPath,
    job: &StackColorJob,
    latest: &LatestStackPreviews,
) -> Option<String> {
    if job.cache_version != STACK_COLOR_CACHE_VERSION
        || job.stacking_version != SEIZA_STACKING_VERSION
    {
        return Some("the color processing version changed".into());
    }
    if !job.sources.iter().all(|source| {
        source_is_current(source, latest)
            && super::fits_path(cache_root, &source.job_id, source.group_index).is_file()
    }) {
        return Some("one or more source channel stacks changed".into());
    }
    if !color_artifacts_exist(cache_root, &job.job_id) {
        return Some("a cached color artifact is missing".into());
    }
    None
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

fn color_original_preview_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(cache_root, job_id).join("preview-original.png")
}

fn color_fits_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    color_dir(cache_root, job_id).join("color.fits")
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
        LatestStackPreviewGroup, StackGroupState, StackGroupStatus,
    };

    fn source_group(filter_name: &str, index: usize) -> LatestStackPreviewGroup {
        LatestStackPreviewGroup {
            job_id: format!("{index:064x}"),
            artifact_revision: format!("rev-{index}"),
            accepted_only: false,
            created_unix_seconds: 10,
            group: StackGroupStatus {
                index,
                target_id: 7,
                target_name: "Color target".into(),
                filter_name: filter_name.into(),
                state: StackGroupState::Ready,
                total_candidates: 3,
                eligible_frames: 3,
                quality_excluded: 0,
                missing_files: 0,
                processed_frames: 3,
                accepted_frames: 3,
                rejected_frames: 0,
                output_channels: 1,
                reference_image_id: Some(1),
                total_exposure_seconds: 180.0,
                preview_url: None,
                fits_url: None,
                error: None,
                input_images: Vec::new(),
                frames: Vec::new(),
            },
        }
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
    fn rejects_palette_shape_mismatches_before_preparing_a_job() {
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Rgb,
            palette: Some(StackNarrowbandPalette::Sho),
            force: false,
        })
        .is_err());
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Lrgb,
            palette: Some(StackNarrowbandPalette::Sho),
            force: false,
        })
        .is_err());
        assert!(validate_request(&StackColorRequest {
            target_id: 1,
            kind: StackColorKind::Narrowband,
            palette: None,
            force: false,
        })
        .is_err());
    }
}
