//! Project-scoped, per-target/per-filter stacking previews.
//!
//! PSF Guard owns frame selection and provenance. `seiza-stacking` owns
//! calibration-free FITS decoding, registration, normalization, admission,
//! and ordered accumulation. Jobs are process-global and run one at a time so
//! a multi-database server cannot multiply the stacker's full-frame buffers.

use axum::{
    body::Body,
    extract::{Path, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use rayon::ThreadPoolBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;
use tokio_util::io::ReaderStream;

use crate::acquisition_context::FramingResolver;
use crate::db::Database;
use crate::models::AcquiredImage;
use crate::sequence_analysis::{
    extract_metrics_from_metadata, ImageQualityResult, SequenceAnalyzer, SequenceAnalyzerConfig,
};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;

pub const SEIZA_STACKING_VERSION: &str = "0.1.0-git-18aa9b8";
/// Bump whenever stack admission, rendering, or persisted artifact semantics
/// change. This deliberately versions PSF Guard policy separately from Seiza.
const STACK_PREVIEW_CACHE_VERSION: u32 = 2;
const MAX_REQUEST_IMAGES: usize = 10_000;
const MAX_REMEMBERED_JOBS: usize = 64;
const PREVIEW_MAX_DIMENSION: u32 = 2400;
const PREVIEW_STRETCH_FACTOR: f64 = 0.2;
const PREVIEW_BLACK_CLIPPING: f64 = -2.8;
const STACK_BYTES_PER_OUTPUT_SAMPLE: u64 = 40;

#[derive(Debug, Clone, Deserialize)]
pub struct StackPreviewRequest {
    pub image_ids: Vec<i32>,
    #[serde(default)]
    pub accepted_only: bool,
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackJobState {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StackGroupState {
    Queued,
    Running,
    Ready,
    Skipped,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackFrameDecision {
    pub image_id: i32,
    pub disposition: String,
    pub reason: Option<String>,
    pub quality_score: Option<f64>,
    pub matched_stars: Option<usize>,
    pub registration_rms_pixels: Option<f64>,
    pub registration_drift_pixels: Option<f64>,
    pub overlap_fraction: Option<f32>,
    pub integrated_fraction: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackGroupStatus {
    pub index: usize,
    pub target_id: i32,
    pub target_name: String,
    pub filter_name: String,
    pub state: StackGroupState,
    pub total_candidates: usize,
    pub eligible_frames: usize,
    pub quality_excluded: usize,
    pub missing_files: usize,
    pub processed_frames: usize,
    pub accepted_frames: usize,
    pub rejected_frames: usize,
    pub reference_image_id: Option<i32>,
    pub total_exposure_seconds: f64,
    pub preview_url: Option<String>,
    pub fits_url: Option<String>,
    pub error: Option<String>,
    pub frames: Vec<StackFrameDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StackPreviewJob {
    pub schema_version: u32,
    pub job_id: String,
    pub database_id: String,
    pub project_id: i32,
    pub state: StackJobState,
    pub accepted_only: bool,
    pub created_unix_seconds: i64,
    #[serde(default)]
    pub artifact_revision: String,
    #[serde(default)]
    pub cache_version: u32,
    pub stacking_version: String,
    pub groups: Vec<StackGroupStatus>,
    pub error: Option<String>,
}

pub struct StackPreviewManager {
    jobs: Mutex<HashMap<String, StackPreviewJob>>,
    permit: Arc<Semaphore>,
}

impl StackPreviewManager {
    pub fn new() -> Self {
        Self {
            jobs: Mutex::new(HashMap::new()),
            permit: Arc::new(Semaphore::new(1)),
        }
    }

    pub fn get(&self, job_id: &str) -> Option<StackPreviewJob> {
        self.jobs.lock().unwrap().get(job_id).cloned()
    }

    fn insert(&self, job: StackPreviewJob) -> bool {
        let mut jobs = self.jobs.lock().unwrap();
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

    fn update(&self, job_id: &str, update: impl FnOnce(&mut StackPreviewJob)) {
        if let Some(job) = self.jobs.lock().unwrap().get_mut(job_id) {
            update(job);
        }
    }
}

impl Default for StackPreviewManager {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone)]
struct PreparedFrame {
    image_id: i32,
    acquired_date: Option<i64>,
    quality_score: Option<f64>,
    path: PathBuf,
}

#[derive(Clone)]
struct PreparedGroup {
    index: usize,
    frames: Vec<PreparedFrame>,
}

struct PreparedJob {
    public: StackPreviewJob,
    groups: Vec<PreparedGroup>,
    cache_root: PathBuf,
}

pub async fn start_stack_previews(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<StackPreviewRequest>,
) -> Result<Json<ApiResponse<StackPreviewJob>>, AppError> {
    validate_request(&request)?;

    let ctx_arc = Arc::clone(&ctx.0);
    let request_for_prepare = request.clone();
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_job(&ctx_arc, project_id, &request_for_prepare)
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Stack preparation task failed: {error}"))
    })??;

    let manifest_path = manifest_path(&prepared.cache_root, &prepared.public.job_id);
    if let Some(existing) = state.stack_previews.get(&prepared.public.job_id)
        && (!request.force
            || matches!(
                existing.state,
                StackJobState::Queued | StackJobState::Running
            ))
    {
        return Ok(Json(ApiResponse::success(existing)));
    }
    if !request.force
        && let Ok(bytes) = std::fs::read(&manifest_path)
        && let Ok(existing) = serde_json::from_slice::<StackPreviewJob>(&bytes)
    {
        let _ = state.stack_previews.insert(existing.clone());
        return Ok(Json(ApiResponse::success(existing)));
    }

    let response = prepared.public.clone();
    if !state.stack_previews.insert(response.clone()) {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_REMEMBERED_JOBS} stack preview jobs may be active at once"
        )));
    }
    enqueue_job(Arc::clone(&state), prepared);
    Ok(Json(ApiResponse::success(response)))
}

pub async fn get_stack_preview_job(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id, job_id)): Path<(String, i32, String)>,
) -> Result<Json<ApiResponse<StackPreviewJob>>, AppError> {
    validate_job_id(&job_id)?;
    if let Some(job) = state.stack_previews.get(&job_id) {
        if job.database_id != ctx.id || job.project_id != project_id {
            return Err(AppError::NotFound);
        }
        return Ok(Json(ApiResponse::success(job)));
    }
    let path = manifest_path(&ctx.cache_dir_path, &job_id);
    let bytes = std::fs::read(path).map_err(|_| AppError::NotFound)?;
    let job: StackPreviewJob = serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid stack manifest: {error}")))?;
    if job.database_id != ctx.id || job.project_id != project_id {
        return Err(AppError::NotFound);
    }
    let _ = state.stack_previews.insert(job.clone());
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_stack_preview_image(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = preview_path(&ctx.cache_dir_path, &job_id, group_index);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "private, max-age=31536000, immutable"),
        ],
        bytes,
    )
        .into_response())
}

pub async fn download_stack_preview_fits(
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
) -> Result<Response, AppError> {
    validate_job_id(&job_id)?;
    let path = fits_path(&ctx.cache_dir_path, &job_id, group_index);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat stack FITS: {error}")))?
        .len();
    let filename = format!("psf-guard-stack-{}-{group_index}.fits", &job_id[..12]);
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
            AppError::InternalError(format!("Failed to build stack FITS response: {error}"))
        })
}

fn validate_request(request: &StackPreviewRequest) -> Result<(), AppError> {
    if request.image_ids.len() < 2 {
        return Err(AppError::BadRequest(
            "Stack previews require at least two image IDs".into(),
        ));
    }
    if request.image_ids.len() > MAX_REQUEST_IMAGES {
        return Err(AppError::BadRequest(format!(
            "Stack preview requests are limited to {MAX_REQUEST_IMAGES} images"
        )));
    }
    let unique = request.image_ids.iter().copied().collect::<HashSet<_>>();
    if unique.len() != request.image_ids.len() {
        return Err(AppError::BadRequest(
            "Stack preview image IDs must be unique".into(),
        ));
    }
    Ok(())
}

fn validate_job_id(job_id: &str) -> Result<(), AppError> {
    if job_id.len() == 64 && job_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Invalid stack preview job ID".into()))
    }
}

fn new_artifact_revision() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    format!("{nanos:x}-{:x}", std::process::id())
}

fn prepare_job(
    ctx: &Arc<DatabaseContext>,
    project_id: i32,
    request: &StackPreviewRequest,
) -> Result<PreparedJob, AppError> {
    let requested = request.image_ids.iter().copied().collect::<HashSet<_>>();
    let (project_images, expected_by_image) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        let images = db
            .get_images_by_project_id(project_id)
            .map_err(AppError::db)?;
        if images.is_empty() {
            return Err(AppError::BadRequest(format!(
                "Project {project_id} has no images"
            )));
        }
        let found = images
            .iter()
            .filter(|(image, _, _)| requested.contains(&image.id))
            .count();
        if found != requested.len() {
            return Err(AppError::BadRequest(
                "Every requested image must belong to the selected project".into(),
            ));
        }
        let selected_groups = images
            .iter()
            .filter(|(image, _, _)| requested.contains(&image.id))
            .map(|(image, _, _)| (image.target_id, image.filter_name.clone()))
            .collect::<HashSet<_>>();
        let relevant = images
            .into_iter()
            .filter(|(image, _, _)| {
                selected_groups.contains(&(image.target_id, image.filter_name.clone()))
            })
            .collect::<Vec<_>>();
        let mut resolver = FramingResolver::new(&conn).map_err(AppError::db)?;
        let expected = relevant
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_for_grading(&conn, image)
                    .map(|value| (image.id, value))
            })
            .collect::<Result<HashMap<_, _>, _>>()
            .map_err(AppError::db)?;
        (relevant, expected)
    };

    let quality = quality_results(ctx, &project_images, &expected_by_image);
    let quality_by_id = quality
        .into_iter()
        .map(|result| (result.image_id, result))
        .collect::<HashMap<_, _>>();

    let mut grouped: BTreeMap<(i32, String, String), Vec<(AcquiredImage, ImageQualityResult)>> =
        BTreeMap::new();
    for (image, _project_name, target_name) in project_images {
        if !requested.contains(&image.id) {
            continue;
        }
        let scored = quality_by_id
            .get(&image.id)
            .cloned()
            .unwrap_or_else(|| fallback_quality(image.id));
        grouped
            .entry((image.target_id, target_name, image.filter_name.clone()))
            .or_default()
            .push((image, scored));
    }

    let mut public_groups = Vec::new();
    let mut prepared_groups = Vec::new();
    let artifact_revision = new_artifact_revision();
    let mut hasher = Sha256::new();
    hasher.update(ctx.id.as_bytes());
    hasher.update(project_id.to_le_bytes());
    hasher.update([request.accepted_only as u8]);
    hasher.update(STACK_PREVIEW_CACHE_VERSION.to_le_bytes());
    hasher.update(SEIZA_STACKING_VERSION.as_bytes());
    hasher.update(PREVIEW_MAX_DIMENSION.to_le_bytes());
    hasher.update(PREVIEW_STRETCH_FACTOR.to_le_bytes());
    hasher.update(PREVIEW_BLACK_CLIPPING.to_le_bytes());

    for (index, ((target_id, target_name, filter_name), mut entries)) in
        grouped.into_iter().enumerate()
    {
        hasher.update(target_id.to_le_bytes());
        hasher.update(target_name.as_bytes());
        hasher.update(filter_name.as_bytes());
        entries.sort_by_key(|(image, _)| (image.acquired_date.unwrap_or(0), image.id));
        let total_candidates = entries.len();
        let mut quality_excluded = 0usize;
        let mut missing_files = 0usize;
        let mut decisions = Vec::new();
        let mut frames = Vec::new();

        for (image, scored) in entries {
            hasher.update(image.id.to_le_bytes());
            hasher.update(image.grading_status.to_le_bytes());
            hasher.update(image.acquired_date.unwrap_or(0).to_le_bytes());
            hasher.update(scored.quality_score.to_le_bytes());
            if let Some(reason) = scored.regrade_reason.as_deref() {
                hasher.update(reason.as_bytes());
            }

            let exclusion = exclusion_reason(&image, &scored, request.accepted_only);
            if let Some(reason) = exclusion {
                quality_excluded += 1;
                decisions.push(excluded_decision(&image, &scored, reason));
                continue;
            }

            let Some(filename) = super::handlers::filename_from_metadata(&image.metadata) else {
                missing_files += 1;
                decisions.push(excluded_decision(
                    &image,
                    &scored,
                    "Metadata has no FITS filename".into(),
                ));
                continue;
            };
            let path = match super::handlers::find_fits_file(ctx, &image, &target_name, &filename) {
                Ok(path) => path,
                Err(_) => {
                    missing_files += 1;
                    decisions.push(excluded_decision(
                        &image,
                        &scored,
                        "FITS file was not found".into(),
                    ));
                    continue;
                }
            };
            hash_source(&mut hasher, &path);
            frames.push(PreparedFrame {
                image_id: image.id,
                acquired_date: image.acquired_date,
                quality_score: Some(scored.quality_score),
                path,
            });
        }

        frames.sort_by(|left, right| {
            right
                .quality_score
                .unwrap_or(0.0)
                .total_cmp(&left.quality_score.unwrap_or(0.0))
                .then_with(|| left.acquired_date.cmp(&right.acquired_date))
                .then_with(|| left.image_id.cmp(&right.image_id))
        });
        let reference_image_id = frames.first().map(|frame| frame.image_id);
        if frames.len() > 1 {
            frames[1..].sort_by_key(|frame| (frame.acquired_date.unwrap_or(0), frame.image_id));
        }
        let eligible_frames = frames.len();
        public_groups.push(StackGroupStatus {
            index,
            target_id,
            target_name,
            filter_name,
            state: if eligible_frames >= 2 {
                StackGroupState::Queued
            } else {
                StackGroupState::Skipped
            },
            total_candidates,
            eligible_frames,
            quality_excluded,
            missing_files,
            processed_frames: 0,
            accepted_frames: 0,
            rejected_frames: 0,
            reference_image_id,
            total_exposure_seconds: 0.0,
            preview_url: None,
            fits_url: None,
            error: (eligible_frames < 2).then(|| "Fewer than two eligible FITS frames".to_string()),
            frames: decisions,
        });
        prepared_groups.push(PreparedGroup { index, frames });
    }

    let digest = hasher.finalize();
    let mut job_id = String::with_capacity(digest.len() * 2);
    for byte in digest {
        write!(&mut job_id, "{byte:02x}").expect("writing to a String cannot fail");
    }
    for group in &mut public_groups {
        if group.state == StackGroupState::Queued {
            group.preview_url = Some(format!(
                "/api/db/{}/stack-previews/{}/{}/preview?v={}",
                ctx.id, job_id, group.index, artifact_revision
            ));
            group.fits_url = Some(format!(
                "/api/db/{}/stack-previews/{}/{}/fits?v={}",
                ctx.id, job_id, group.index, artifact_revision
            ));
        }
    }
    let now = chrono::Utc::now().timestamp();
    Ok(PreparedJob {
        public: StackPreviewJob {
            schema_version: 1,
            job_id,
            database_id: ctx.id.clone(),
            project_id,
            state: StackJobState::Queued,
            accepted_only: request.accepted_only,
            created_unix_seconds: now,
            artifact_revision,
            cache_version: STACK_PREVIEW_CACHE_VERSION,
            stacking_version: SEIZA_STACKING_VERSION.into(),
            groups: public_groups,
            error: None,
        },
        groups: prepared_groups,
        cache_root: ctx.cache_dir_path.clone(),
    })
}

fn exclusion_reason(
    image: &AcquiredImage,
    scored: &ImageQualityResult,
    accepted_only: bool,
) -> Option<String> {
    if image.grading_status == 2 {
        Some("Database grade is Rejected".to_string())
    } else if accepted_only && image.grading_status != 1 {
        Some("Accepted-only policy excludes Pending images".to_string())
    } else {
        scored.regrade_reason.clone()
    }
}

fn quality_results(
    ctx: &DatabaseContext,
    images: &[(AcquiredImage, String, String)],
    expected_by_image: &HashMap<i32, Option<(f64, f64)>>,
) -> Vec<ImageQualityResult> {
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let mut grouped: BTreeMap<(i32, String, String), Vec<&AcquiredImage>> = BTreeMap::new();
    for (image, _, target_name) in images {
        grouped
            .entry((
                image.target_id,
                target_name.clone(),
                image.filter_name.clone(),
            ))
            .or_default()
            .push(image);
    }
    let config = SequenceAnalyzerConfig::default();
    let session_gap = config.session_gap_minutes;
    let analyzer = SequenceAnalyzer::new(config);
    let mut output = Vec::new();
    for ((target_id, target_name, filter_name), group) in grouped {
        let mut metrics = Vec::with_capacity(group.len());
        let mut entries = Vec::with_capacity(group.len());
        for image in group {
            let mut value =
                extract_metrics_from_metadata(image.id, &image.metadata, image.acquired_date);
            super::handlers::merge_spatial_metrics(
                &mut value,
                &ctx.spatial_metrics,
                &image.metadata,
            );
            super::handlers::merge_astrometry_metrics(
                &mut value,
                &ctx.cache_dir_path,
                &image.metadata,
                &ctx.astrometry_evidence,
                expected_by_image.get(&image.id).copied().flatten(),
            );
            entries.push(super::handlers::stored_entry_for(
                &ctx.spatial_metrics,
                image.id,
                &image.metadata,
            ));
            metrics.push(value);
        }
        super::handlers::merge_photometric_signals(&mut metrics, &entries, session_gap);
        for sequence in analyzer.analyze(&metrics, target_id, &target_name, &filter_name) {
            output.extend(sequence.images);
        }
    }
    output
}

fn fallback_quality(image_id: i32) -> ImageQualityResult {
    use crate::sequence_analysis::NormalizedMetrics;

    ImageQualityResult {
        image_id,
        quality_score: 1.0,
        temporal_anomaly_score: 0.0,
        category: None,
        flags: Vec::new(),
        normalized_metrics: NormalizedMetrics {
            star_count: None,
            hfr: None,
            eccentricity: None,
            snr: None,
            background: None,
            spatial_coverage: None,
            transparency: None,
            pointing: None,
        },
        regrade_reason: None,
        pointing: None,
        satellite: None,
        details: None,
    }
}

fn excluded_decision(
    image: &AcquiredImage,
    scored: &ImageQualityResult,
    reason: String,
) -> StackFrameDecision {
    StackFrameDecision {
        image_id: image.id,
        disposition: "excluded".into(),
        reason: Some(reason),
        quality_score: Some(scored.quality_score),
        matched_stars: None,
        registration_rms_pixels: None,
        registration_drift_pixels: None,
        overlap_fraction: None,
        integrated_fraction: None,
    }
}

fn hash_source(hasher: &mut Sha256, path: &FsPath) {
    hasher.update(path.to_string_lossy().as_bytes());
    if let Ok(metadata) = path.metadata() {
        hasher.update(metadata.len().to_le_bytes());
        if let Ok(modified) = metadata.modified()
            && let Ok(duration) = modified.duration_since(UNIX_EPOCH)
        {
            hasher.update(duration.as_secs().to_le_bytes());
            hasher.update(duration.subsec_nanos().to_le_bytes());
        }
    }
}

fn enqueue_job(state: Arc<AppState>, prepared: PreparedJob) {
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
            run_job(&state_for_job, prepared)
        })
        .await;
        if let Err(error) = result {
            state.stack_previews.update(&job_id, |job| {
                job.state = StackJobState::Failed;
                job.error = Some(format!("Stack worker panicked: {error}"));
            });
        }
    });
}

fn run_job(state: &Arc<AppState>, prepared: PreparedJob) {
    let job_id = prepared.public.job_id.clone();
    let PreparedJob {
        public: _,
        groups,
        cache_root,
    } = prepared;
    state.stack_previews.update(&job_id, |job| {
        job.state = StackJobState::Running;
    });
    let worker_policy = state.worker_policy();
    let run = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        for group in groups {
            if group.frames.len() < 2 {
                continue;
            }
            state.stack_previews.update(&job_id, |job| {
                job.groups[group.index].state = StackGroupState::Running;
            });
            let result = run_group(state, &job_id, &cache_root, group.clone(), &worker_policy);
            state.stack_previews.update(&job_id, |job| match result {
                Ok(()) => job.groups[group.index].state = StackGroupState::Ready,
                Err(error) => {
                    job.groups[group.index].state = StackGroupState::Error;
                    job.groups[group.index].error = Some(error);
                }
            });
        }
    }));
    state.stack_previews.update(&job_id, |job| match run {
        Ok(()) => job.state = StackJobState::Completed,
        Err(_) => {
            job.state = StackJobState::Failed;
            job.error = Some("Stack worker panicked".into());
        }
    });
    if let Some(job) = state.stack_previews.get(&job_id)
        && let Err(error) = persist_manifest(&cache_root, &job)
    {
        tracing::warn!("Failed to persist stack preview manifest: {error}");
    }
}

fn run_group(
    state: &Arc<AppState>,
    job_id: &str,
    cache_root: &FsPath,
    group: PreparedGroup,
    worker_policy: &crate::concurrency::WorkerPolicy,
) -> Result<(), String> {
    use seiza_stacking::{
        CalibrationMasters, FitsFrame, FrameDisposition, LiveStacker, NormalizationMode,
        StackOptions,
    };

    let reference_frame =
        FitsFrame::open(&group.frames[0].path).map_err(|error| error.to_string())?;
    let output_channels = if reference_frame.bayer.is_some() {
        3_u64
    } else {
        reference_frame.image.channels as u64
    };
    let pixels = reference_frame.image.pixel_count();
    let estimate = (pixels as u64)
        .saturating_mul(output_channels)
        .saturating_mul(STACK_BYTES_PER_OUTPUT_SAMPLE);
    if let Some(available) = crate::concurrency::available_memory_bytes()
        && estimate > (available as f64 * worker_policy.memory_budget_fraction) as u64
    {
        return Err(format!(
            "Estimated stack memory {} MiB exceeds the configured available-memory budget",
            estimate / (1024 * 1024)
        ));
    }
    let budget = crate::concurrency::plan_workers(
        None,
        worker_policy,
        crate::concurrency::Priority::Interactive,
        Some(pixels),
    );
    let pool = ThreadPoolBuilder::new()
        .num_threads(budget.workers)
        .thread_name(|index| format!("stack-preview-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Stack preview {} group {}: {} worker(s) — {}",
        job_id,
        group.index,
        budget.workers,
        budget.rationale
    );

    pool.install(|| {
        let reference_exposure = reference_frame.exposure_seconds.unwrap_or(0.0);
        let options = StackOptions {
            normalization: NormalizationMode::Global,
            ..StackOptions::default()
        };
        let mut stacker = LiveStacker::new(reference_frame, CalibrationMasters::default(), options)
            .map_err(|error| error.to_string())?;
        state.stack_previews.update(job_id, |job| {
            let status = &mut job.groups[group.index];
            status.processed_frames = 1;
            status.accepted_frames = 1;
            status.total_exposure_seconds = reference_exposure;
            status.frames.push(StackFrameDecision {
                image_id: group.frames[0].image_id,
                disposition: "reference".into(),
                reason: None,
                quality_score: group.frames[0].quality_score,
                matched_stars: None,
                registration_rms_pixels: None,
                registration_drift_pixels: None,
                overlap_fraction: Some(1.0),
                integrated_fraction: Some(1.0),
            });
        });

        for frame in group.frames.iter().skip(1) {
            let opened = FitsFrame::open(&frame.path);
            let exposure = opened
                .as_ref()
                .ok()
                .and_then(|value| value.exposure_seconds)
                .unwrap_or(0.0);
            let decision = match opened {
                Ok(opened) => match stacker.push(opened).map_err(|error| error.to_string())? {
                    FrameDisposition::Accepted(diagnostics) => StackFrameDecision {
                        image_id: frame.image_id,
                        disposition: "accepted".into(),
                        reason: None,
                        quality_score: frame.quality_score,
                        matched_stars: Some(diagnostics.matched_stars),
                        registration_rms_pixels: Some(diagnostics.registration_rms_pixels),
                        registration_drift_pixels: Some(diagnostics.registration_drift_pixels),
                        overlap_fraction: Some(diagnostics.overlap_fraction),
                        integrated_fraction: Some(diagnostics.integrated_fraction),
                    },
                    FrameDisposition::Rejected(reason) => StackFrameDecision {
                        image_id: frame.image_id,
                        disposition: "rejected".into(),
                        reason: Some(reason.to_string()),
                        quality_score: frame.quality_score,
                        matched_stars: None,
                        registration_rms_pixels: None,
                        registration_drift_pixels: None,
                        overlap_fraction: None,
                        integrated_fraction: None,
                    },
                },
                Err(error) => StackFrameDecision {
                    image_id: frame.image_id,
                    disposition: "rejected".into(),
                    reason: Some(error.to_string()),
                    quality_score: frame.quality_score,
                    matched_stars: None,
                    registration_rms_pixels: None,
                    registration_drift_pixels: None,
                    overlap_fraction: None,
                    integrated_fraction: None,
                },
            };
            state.stack_previews.update(job_id, |job| {
                let status = &mut job.groups[group.index];
                status.processed_frames += 1;
                if matches!(decision.disposition.as_str(), "accepted") {
                    status.accepted_frames += 1;
                    status.total_exposure_seconds += exposure;
                } else {
                    status.rejected_frames += 1;
                }
                status.frames.push(decision);
            });
        }
        let reference_headers = stacker.reference_headers().to_vec();
        let snapshot = stacker.into_snapshot().map_err(|error| error.to_string())?;
        let fits_destination = fits_path(cache_root, job_id, group.index);
        let fits_parent = fits_destination
            .parent()
            .ok_or_else(|| "Stack FITS path has no parent".to_string())?;
        std::fs::create_dir_all(fits_parent).map_err(|error| error.to_string())?;
        let fits_temporary =
            fits_destination.with_extension(format!("{}.tmp.fits", std::process::id()));
        seiza_stacking::write_fits_f32(&fits_temporary, &snapshot, &reference_headers)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&fits_temporary, &fits_destination).map_err(|error| error.to_string())?;
        render_preview_atomic(
            &snapshot.image,
            &preview_path(cache_root, job_id, group.index),
        )
    })
}

fn render_preview_atomic(
    image: &seiza_stacking::LinearImage,
    destination: &FsPath,
) -> Result<(), String> {
    let parent = destination
        .parent()
        .ok_or_else(|| "Stack preview path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let pixels = crate::mtf_stretch::stretch_f32_to_u8(
        &image.data,
        PREVIEW_STRETCH_FACTOR,
        PREVIEW_BLACK_CLIPPING,
    )
    .ok_or_else(|| "Stack has no usable finite sample range".to_string())?;
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
    let resized = dynamic.resize(
        PREVIEW_MAX_DIMENSION,
        PREVIEW_MAX_DIMENSION,
        image::imageops::FilterType::Lanczos3,
    );
    let temporary = destination.with_extension(format!("{}.tmp.png", std::process::id()));
    resized
        .save(&temporary)
        .map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, destination).map_err(|error| error.to_string())
}

fn persist_manifest(cache_root: &FsPath, job: &StackPreviewJob) -> Result<(), String> {
    let path = manifest_path(cache_root, &job.job_id);
    let parent = path
        .parent()
        .ok_or_else(|| "Stack manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(job).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

fn stack_dir(cache_root: &FsPath, job_id: &str) -> PathBuf {
    cache_root.join("stack-previews").join(job_id)
}

fn manifest_path(cache_root: &FsPath, job_id: &str) -> PathBuf {
    stack_dir(cache_root, job_id).join("manifest.json")
}

fn preview_path(cache_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(cache_root, job_id).join(format!("group-{group_index}.png"))
}

fn fits_path(cache_root: &FsPath, job_id: &str, group_index: usize) -> PathBuf {
    stack_dir(cache_root, job_id).join(format!("group-{group_index}.fits"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_requires_unique_pair_or_more() {
        assert!(validate_request(&StackPreviewRequest {
            image_ids: vec![1],
            accepted_only: false,
            force: false,
        })
        .is_err());
        assert!(validate_request(&StackPreviewRequest {
            image_ids: vec![1, 1],
            accepted_only: false,
            force: false,
        })
        .is_err());
        assert!(validate_request(&StackPreviewRequest {
            image_ids: vec![1, 2],
            accepted_only: false,
            force: false,
        })
        .is_ok());
    }

    #[test]
    fn artifact_paths_are_namespaced_by_job_and_group() {
        assert_eq!(
            preview_path(FsPath::new("/cache/db"), "abc", 2),
            PathBuf::from("/cache/db/stack-previews/abc/group-2.png")
        );
        assert_eq!(
            fits_path(FsPath::new("/cache/db"), "abc", 2),
            PathBuf::from("/cache/db/stack-previews/abc/group-2.fits")
        );
    }

    #[test]
    fn artifact_revisions_are_safe_cache_busters() {
        let revision = new_artifact_revision();
        assert!(!revision.is_empty());
        assert!(revision
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() || byte == b'-'));
    }

    #[test]
    fn job_ids_must_be_sha256_hex() {
        assert!(validate_job_id(&"a".repeat(64)).is_ok());
        assert!(validate_job_id(&"A".repeat(64)).is_ok());
        assert!(validate_job_id("../manifest.json").is_err());
        assert!(validate_job_id(&"g".repeat(64)).is_err());
    }

    #[test]
    fn selection_policy_keeps_regrades_and_database_grades_authoritative() {
        let mut image = AcquiredImage {
            id: 7,
            project_id: 1,
            target_id: 2,
            acquired_date: Some(123),
            filter_name: "Ha".into(),
            grading_status: 0,
            metadata: "{}".into(),
            reject_reason: None,
            profile_id: None,
            guid: None,
        };
        let mut quality = fallback_quality(image.id);

        assert!(exclusion_reason(&image, &quality, false).is_none());
        assert!(exclusion_reason(&image, &quality, true)
            .unwrap()
            .contains("Accepted-only"));

        quality.regrade_reason = Some("[Auto] Off target".into());
        assert_eq!(
            exclusion_reason(&image, &quality, false).as_deref(),
            Some("[Auto] Off target")
        );

        image.grading_status = 2;
        assert_eq!(
            exclusion_reason(&image, &quality, false).as_deref(),
            Some("Database grade is Rejected")
        );
    }
}
