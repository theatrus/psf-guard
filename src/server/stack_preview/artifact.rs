//! User-directed source-frame search for a suspicious stack region.

use super::{
    color::{self, StackColorJob},
    source_fingerprint, StackGroupState, StackGroupStatus, StackJobState, StackPreviewJob,
    StackPreviewManager, MAX_REMEMBERED_JOBS,
};
use axum::{
    body::Body,
    extract::{Path, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_LENGTH, CONTENT_TYPE},
        StatusCode,
    },
    response::Response,
    Json,
};
use seiza_stacking::{
    resample_region_to_reference, FitsFrame, ReferenceRegion, SimilarityTransform,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt::Write as _,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};
use tokio_util::io::ReaderStream;

use crate::{
    db::Database,
    server::{api::ApiResponse, extract::DbContext, handlers::AppError, state::AppState},
};

const ARTIFACT_SEARCH_CACHE_VERSION: u32 = 1;
const MIN_REGION_EDGE: usize = 8;
const MAX_REGION_EDGE: usize = 512;
const MAX_ANALYSIS_SAMPLES: usize = 65_536;
const OUTLIER_SIGMA: f32 = 5.0;

#[derive(Debug, Clone, Deserialize)]
pub struct ArtifactSearchRequest {
    pub artifact_revision: String,
    pub region: ReferenceRegion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactSearchState {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSearchResult {
    pub image_id: i32,
    pub filter_name: String,
    pub acquired_unix_seconds: Option<i64>,
    pub grading_status: i32,
    pub score: f32,
    pub peak_sigma: f32,
    pub bright_fraction: f32,
    pub dark_fraction: f32,
    pub coverage_fraction: f32,
    pub evidence: String,
    pub direction: String,
    pub crop_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactSearchJob {
    pub schema_version: u32,
    pub search_id: String,
    pub database_id: String,
    pub source_job_id: String,
    pub source_kind: String,
    pub group_index: Option<usize>,
    pub artifact_revision: String,
    pub region: ReferenceRegion,
    pub state: ArtifactSearchState,
    pub phase: String,
    pub total_frames: usize,
    pub processed_frames: usize,
    pub created_unix_seconds: i64,
    #[serde(default)]
    pub notes: Vec<String>,
    pub results: Vec<ArtifactSearchResult>,
    pub error: Option<String>,
}

#[derive(Clone)]
struct SearchSource {
    image_id: i32,
    filter_name: String,
    acquired_unix_seconds: Option<i64>,
    grading_status: i32,
    path: PathBuf,
    transform: SimilarityTransform,
    normalization: Option<seiza_stacking::NormalizationMap>,
    fingerprint: String,
}

#[derive(Clone)]
struct SearchGroup {
    label: String,
    expected_calibration_fingerprint: String,
    sources: Vec<SearchSource>,
}

struct PreparedSearch {
    public: ArtifactSearchJob,
    groups: Vec<SearchGroup>,
    reference_width: usize,
    reference_height: usize,
    cache_root: PathBuf,
}

struct LoadedCrop {
    source: SearchSource,
    analysis: Vec<f32>,
}

impl StackPreviewManager {
    fn get_artifact_search(&self, search_id: &str) -> Option<ArtifactSearchJob> {
        self.artifact_jobs.lock().unwrap().get(search_id).cloned()
    }

    fn insert_artifact_search(&self, job: ArtifactSearchJob) -> bool {
        let mut jobs = self.artifact_jobs.lock().unwrap();
        if jobs.len() >= MAX_REMEMBERED_JOBS && !jobs.contains_key(&job.search_id) {
            let Some(oldest) = jobs
                .values()
                .filter(|entry| {
                    matches!(
                        entry.state,
                        ArtifactSearchState::Completed | ArtifactSearchState::Failed
                    )
                })
                .min_by_key(|entry| entry.created_unix_seconds)
                .map(|entry| entry.search_id.clone())
            else {
                return false;
            };
            jobs.remove(&oldest);
        }
        jobs.insert(job.search_id.clone(), job);
        true
    }

    fn update_artifact_search(&self, search_id: &str, update: impl FnOnce(&mut ArtifactSearchJob)) {
        if let Some(job) = self.artifact_jobs.lock().unwrap().get_mut(search_id) {
            update(job);
        }
    }
}

pub async fn start_mono_artifact_search(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id, group_index)): Path<(String, String, usize)>,
    Json(request): Json<ArtifactSearchRequest>,
) -> Result<Json<ApiResponse<ArtifactSearchJob>>, AppError> {
    super::validate_job_id(&job_id)?;
    validate_region(request.region)?;
    let stack = load_stack_job(&state, &ctx, &job_id)?;
    if stack.database_id != ctx.id {
        return Err(AppError::NotFound);
    }
    if stack.artifact_revision != request.artifact_revision {
        return Err(AppError::Conflict(
            "The displayed stack was rebuilt; reopen it before searching".into(),
        ));
    }
    let group = stack
        .groups
        .get(group_index)
        .filter(|group| group.index == group_index && group.state == StackGroupState::Ready)
        .ok_or(AppError::NotFound)?
        .clone();
    let ctx_arc = Arc::clone(&ctx.0);
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_search(
            &ctx_arc,
            &job_id,
            "mono",
            Some(group_index),
            &request,
            vec![(group, SimilarityTransform::IDENTITY, None)],
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Artifact preparation failed: {error}")))??;
    start_prepared_search(state, prepared)
}

pub async fn start_color_artifact_search(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job_id)): Path<(String, String)>,
    Json(request): Json<ArtifactSearchRequest>,
) -> Result<Json<ApiResponse<ArtifactSearchJob>>, AppError> {
    super::validate_job_id(&job_id)?;
    validate_region(request.region)?;
    let color_job = load_color_job(&state, &ctx, &job_id)?;
    if color_job.database_id != ctx.id || color_job.state != StackJobState::Completed {
        return Err(AppError::NotFound);
    }
    if color_job.artifact_revision != request.artifact_revision {
        return Err(AppError::Conflict(
            "The displayed color preview was rebuilt; reopen it before searching".into(),
        ));
    }
    let stack_sources = load_color_stack_sources(&state, &ctx, &color_job)?;
    let ctx_arc = Arc::clone(&ctx.0);
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_search(&ctx_arc, &job_id, "color", None, &request, stack_sources)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Artifact preparation failed: {error}")))??;
    start_prepared_search(state, prepared)
}

pub async fn get_artifact_search(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, search_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<ArtifactSearchJob>>, AppError> {
    validate_search_id(&search_id)?;
    let job = load_artifact_job(&state, &ctx, &search_id)?;
    if job.database_id != ctx.id {
        return Err(AppError::NotFound);
    }
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_artifact_crop(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, search_id, image_id)): Path<(String, String, i32)>,
) -> Result<Response, AppError> {
    validate_search_id(&search_id)?;
    let job = load_artifact_job(&state, &ctx, &search_id)?;
    if job.database_id != ctx.id || !job.results.iter().any(|result| result.image_id == image_id) {
        return Err(AppError::NotFound);
    }
    let path = artifact_crop_path(&ctx.cache_dir_path, &search_id, image_id);
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat artifact crop: {error}")))?
        .len();
    Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, "image/png")
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable")
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| AppError::InternalError(format!("Failed to serve artifact crop: {error}")))
}

fn start_prepared_search(
    state: Arc<AppState>,
    prepared: PreparedSearch,
) -> Result<Json<ApiResponse<ArtifactSearchJob>>, AppError> {
    if let Some(existing) = state
        .stack_previews
        .get_artifact_search(&prepared.public.search_id)
        && matches!(
            existing.state,
            ArtifactSearchState::Queued
                | ArtifactSearchState::Running
                | ArtifactSearchState::Completed
        )
    {
        return Ok(Json(ApiResponse::success(existing)));
    }
    if let Ok(bytes) = std::fs::read(artifact_manifest_path(
        &prepared.cache_root,
        &prepared.public.search_id,
    )) && let Ok(existing) = serde_json::from_slice::<ArtifactSearchJob>(&bytes)
        && existing.state == ArtifactSearchState::Completed
    {
        let _ = state
            .stack_previews
            .insert_artifact_search(existing.clone());
        return Ok(Json(ApiResponse::success(existing)));
    }
    let response = prepared.public.clone();
    if !state
        .stack_previews
        .insert_artifact_search(response.clone())
    {
        return Err(AppError::BadRequest(format!(
            "At most {MAX_REMEMBERED_JOBS} artifact searches may be active at once"
        )));
    }
    enqueue_search(state, prepared);
    Ok(Json(ApiResponse::success(response)))
}

fn load_stack_job(
    state: &Arc<AppState>,
    ctx: &DbContext,
    job_id: &str,
) -> Result<StackPreviewJob, AppError> {
    if let Some(job) = state.stack_previews.get(job_id) {
        return Ok(job);
    }
    let bytes = std::fs::read(super::manifest_path(&ctx.cache_dir_path, job_id))
        .map_err(|_| AppError::NotFound)?;
    serde_json::from_slice(&bytes)
        .map_err(|error| AppError::InternalError(format!("Invalid stack manifest: {error}")))
}

fn load_color_job(
    state: &Arc<AppState>,
    ctx: &DbContext,
    job_id: &str,
) -> Result<StackColorJob, AppError> {
    state
        .stack_previews
        .get_color(job_id)
        .map(Ok)
        .unwrap_or_else(|| color::load_persisted_color_job(&ctx.cache_dir_path, job_id))
}

fn load_color_stack_sources(
    state: &Arc<AppState>,
    ctx: &DbContext,
    color_job: &StackColorJob,
) -> Result<Vec<(StackGroupStatus, SimilarityTransform, Option<String>)>, AppError> {
    color_job
        .sources
        .iter()
        .map(|source| {
            let transform = source.registration_transform.ok_or_else(|| {
                AppError::BadRequest(format!(
                    "Rebuild the color preview before searching {}; its registration transform was not retained",
                    source.role.label()
                ))
            })?;
            let stack = load_stack_job(state, ctx, &source.job_id)?;
            if stack.artifact_revision != source.artifact_revision {
                return Err(AppError::Conflict(format!(
                    "The {} channel stack changed; rebuild the color preview",
                    source.role.label()
                )));
            }
            let group = stack
                .groups
                .get(source.group_index)
                .filter(|group| {
                    group.index == source.group_index && group.state == StackGroupState::Ready
                })
                .ok_or(AppError::NotFound)?
                .clone();
            Ok((group, transform, Some(source.role.label().to_string())))
        })
        .collect()
}

fn prepare_search(
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    source_job_id: &str,
    source_kind: &str,
    group_index: Option<usize>,
    request: &ArtifactSearchRequest,
    stack_groups: Vec<(StackGroupStatus, SimilarityTransform, Option<String>)>,
) -> Result<PreparedSearch, AppError> {
    let reference_path = if source_kind == "mono" {
        super::original_preview_path(&ctx.cache_dir_path, source_job_id, group_index.unwrap_or(0))
    } else {
        color::color_original_preview_path(&ctx.cache_dir_path, source_job_id)
    };
    let reference_reader = image::ImageReader::open(reference_path).map_err(|error| {
        AppError::InternalError(format!("Failed to read the stack dimensions: {error}"))
    })?;
    let (reference_width, reference_height) =
        reference_reader.into_dimensions().map_err(|error| {
            AppError::InternalError(format!("Failed to read the stack dimensions: {error}"))
        })?;
    let reference_width = reference_width as usize;
    let reference_height = reference_height as usize;
    validate_region_bounds(request.region, reference_width, reference_height)?;

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);
    let mut groups = Vec::new();
    let mut notes = Vec::new();
    let mut hasher = Sha256::new();
    hasher.update(ctx.id.as_bytes());
    hasher.update(source_job_id.as_bytes());
    hasher.update(source_kind.as_bytes());
    hasher.update(request.artifact_revision.as_bytes());
    hasher.update(ARTIFACT_SEARCH_CACHE_VERSION.to_le_bytes());
    hash_region(&mut hasher, request.region);

    for (group, output_transform, role_label) in stack_groups {
        let label = role_label.unwrap_or_else(|| {
            if group.filter_name.is_empty() {
                "No filter".into()
            } else {
                group.filter_name.clone()
            }
        });
        let decisions = group
            .frames
            .iter()
            .filter(|frame| matches!(frame.disposition.as_str(), "reference" | "accepted"))
            .cloned()
            .collect::<Vec<_>>();
        if decisions.len() < 3 {
            notes.push(format!(
                "{label} has only {} integrated frames; at least three are needed to rank a suspect",
                decisions.len()
            ));
            continue;
        }
        let ids = decisions
            .iter()
            .map(|frame| frame.image_id)
            .collect::<Vec<_>>();
        let images = db
            .get_images_by_ids(&ids)
            .map_err(AppError::db)?
            .into_iter()
            .map(|image| (image.id, image))
            .collect::<HashMap<_, _>>();
        if images.len() != ids.len() {
            return Err(AppError::Conflict(
                "One or more stack inputs are no longer in the database".into(),
            ));
        }
        let mut sources = Vec::with_capacity(decisions.len());
        for decision in decisions {
            let image = images.get(&decision.image_id).ok_or(AppError::NotFound)?;
            let filename = super::super::handlers::filename_from_metadata(&image.metadata)
                .ok_or_else(|| {
                    AppError::BadRequest("Stack input metadata has no FITS filename".into())
                })?;
            let path =
                super::super::handlers::find_fits_file(ctx, image, &group.target_name, &filename)?;
            let fingerprint = source_fingerprint(&path);
            if decision.source_fingerprint.as_deref() != Some(fingerprint.as_str()) {
                return Err(AppError::Conflict(format!(
                    "Image {} changed after this stack was built; rebuild the stack before searching",
                    image.id
                )));
            }
            let transform = decision.registration_transform.ok_or_else(|| {
                AppError::BadRequest(
                    "Rebuild this stack before searching; its frame transforms were not retained"
                        .into(),
                )
            })?;
            let normalization = if decision.disposition == "reference" {
                None
            } else {
                Some(decision.normalization_map.ok_or_else(|| {
                    AppError::BadRequest(
                        "Rebuild this stack before searching; its frame normalization was not retained"
                            .into(),
                    )
                })?)
            };
            let source = SearchSource {
                image_id: image.id,
                filter_name: if group.filter_name.is_empty() {
                    label.clone()
                } else {
                    group.filter_name.clone()
                },
                acquired_unix_seconds: image.acquired_date,
                grading_status: image.grading_status,
                path,
                transform: transform.then(output_transform),
                normalization,
                fingerprint,
            };
            hasher.update(source.image_id.to_le_bytes());
            hasher.update(source.fingerprint.as_bytes());
            hasher.update(serde_json::to_vec(&source.transform).map_err(|error| {
                AppError::InternalError(format!("Failed to encode frame transform: {error}"))
            })?);
            hasher.update(serde_json::to_vec(&source.normalization).map_err(|error| {
                AppError::InternalError(format!("Failed to encode frame normalization: {error}"))
            })?);
            sources.push(source);
        }
        groups.push(SearchGroup {
            label,
            expected_calibration_fingerprint: group.calibration.fingerprint,
            sources,
        });
    }
    if groups.is_empty() {
        return Err(AppError::BadRequest(notes.first().cloned().unwrap_or_else(
            || "No integrated source frames can be searched".into(),
        )));
    }
    let search_id = hex_digest(hasher.finalize());
    let total_frames = groups.iter().map(|group| group.sources.len()).sum();
    Ok(PreparedSearch {
        public: ArtifactSearchJob {
            schema_version: 1,
            search_id,
            database_id: ctx.id.clone(),
            source_job_id: source_job_id.into(),
            source_kind: source_kind.into(),
            group_index,
            artifact_revision: request.artifact_revision.clone(),
            region: request.region,
            state: ArtifactSearchState::Queued,
            phase: "Waiting for the stack processor".into(),
            total_frames,
            processed_frames: 0,
            created_unix_seconds: chrono::Utc::now().timestamp(),
            notes,
            results: Vec::new(),
            error: None,
        },
        groups,
        reference_width,
        reference_height,
        cache_root: ctx.cache_dir_path.clone(),
    })
}

fn enqueue_search(state: Arc<AppState>, prepared: PreparedSearch) {
    let permit = Arc::clone(&state.stack_previews.permit);
    tokio::spawn(async move {
        let Ok(_permit) = permit.acquire_owned().await else {
            return;
        };
        let guard = state.begin_interactive_job();
        let state_for_job = Arc::clone(&state);
        let search_id = prepared.public.search_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            run_search(&state_for_job, prepared)
        })
        .await;
        if let Err(error) = result {
            state
                .stack_previews
                .update_artifact_search(&search_id, |job| {
                    job.state = ArtifactSearchState::Failed;
                    job.error = Some(format!("Artifact search worker panicked: {error}"));
                });
        }
    });
}

fn run_search(state: &Arc<AppState>, prepared: PreparedSearch) {
    let search_id = prepared.public.search_id.clone();
    let database_id = prepared.public.database_id.clone();
    let cache_root = prepared.cache_root.clone();
    state
        .stack_previews
        .update_artifact_search(&search_id, |job| {
            job.state = ArtifactSearchState::Running;
            job.phase = "Preparing source frames".into();
        });
    let result = run_search_inner(state, &prepared);
    state
        .stack_previews
        .update_artifact_search(&search_id, |job| match result {
            Ok(results) => {
                job.results = results;
                job.state = ArtifactSearchState::Completed;
                job.phase = "Source-frame search complete".into();
            }
            Err(error) => {
                job.state = ArtifactSearchState::Failed;
                job.phase = "Source-frame search failed".into();
                job.error = Some(error);
            }
        });
    if let Some(job) = state.stack_previews.get_artifact_search(&search_id)
        && let Err(error) = persist_artifact_job(&cache_root, &job)
    {
        tracing::warn!("Failed to persist artifact search {search_id}: {error}");
    }
    tracing::info!("Artifact search {search_id} finished for database {database_id}");
}

fn run_search_inner(
    state: &Arc<AppState>,
    prepared: &PreparedSearch,
) -> Result<Vec<ArtifactSearchResult>, String> {
    let ctx = state
        .get_database(&prepared.public.database_id)
        .ok_or_else(|| "The database is no longer configured".to_string())?;
    let calibration_conn = rusqlite::Connection::open(&ctx.database_path)
        .map_err(|error| format!("Opening calibration catalog: {error}"))?;
    calibration_conn
        .busy_timeout(std::time::Duration::from_secs(60))
        .map_err(|error| format!("Configuring calibration catalog: {error}"))?;
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| format!("Indexing calibration folders: {error}"))?;
    let mut all_results = Vec::new();
    let output_dir = artifact_dir(&prepared.cache_root, &prepared.public.search_id);
    std::fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;

    for group in &prepared.groups {
        state
            .stack_previews
            .update_artifact_search(&prepared.public.search_id, |job| {
                job.phase = format!("Scanning {} source frames", group.label);
            });
        let paths = group
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>();
        let (masters, applied) = crate::calibration::resolve_or_build_masters_for_group(
            &calibration_conn,
            &prepared.cache_root,
            &paths,
            Some(&directory_tree),
        )
        .map_err(|error| error.to_string())?;
        if applied.fingerprint != group.expected_calibration_fingerprint {
            return Err(format!(
                "The calibration selected for {} changed; rebuild the stack before searching",
                group.label
            ));
        }
        let mut crops = Vec::with_capacity(group.sources.len());
        for source in &group.sources {
            if source_fingerprint(&source.path) != source.fingerprint {
                return Err(format!(
                    "Image {} changed while the source-frame search was waiting; rebuild the stack before searching",
                    source.image_id
                ));
            }
            let mut frame = FitsFrame::open(&source.path).map_err(|error| error.to_string())?;
            masters
                .apply(&mut frame.image, frame.exposure_seconds, frame.bayer)
                .map_err(|error| error.to_string())?;
            let frame = frame.into_prepared().map_err(|error| error.to_string())?;
            let mut crop = resample_region_to_reference(
                &frame.image,
                prepared.reference_width,
                prepared.reference_height,
                prepared.public.region,
                source.transform,
            )
            .map_err(|error| error.to_string())?;
            if let Some(normalization) = &source.normalization {
                normalization
                    .apply_global(&mut crop)
                    .map_err(|error| error.to_string())?;
            }
            let original_path = artifact_crop_path(
                &prepared.cache_root,
                &prepared.public.search_id,
                source.image_id,
            );
            super::stretch::render_image_preview_atomic(
                &crop,
                &super::stretch::default_linear_config(),
                super::stretch::StackStretchSourceTransfer::Linear,
                &original_path,
            )?;
            let analysis = sampled_luminance(&crop);
            crops.push(LoadedCrop {
                source: source.clone(),
                analysis,
            });
            state
                .stack_previews
                .update_artifact_search(&prepared.public.search_id, |job| {
                    job.processed_frames += 1
                });
        }
        all_results.extend(score_group(
            &prepared.public.database_id,
            &prepared.public.search_id,
            crops,
        )?);
    }
    all_results.sort_by(|left, right| {
        left.filter_name
            .cmp(&right.filter_name)
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.image_id.cmp(&right.image_id))
    });
    Ok(all_results)
}

fn sampled_luminance(image: &seiza_stacking::LinearImage) -> Vec<f32> {
    let luminance = image.luminance();
    let step = luminance.len().div_ceil(MAX_ANALYSIS_SAMPLES).max(1);
    luminance.into_iter().step_by(step).collect()
}

fn score_group(
    database_id: &str,
    search_id: &str,
    crops: Vec<LoadedCrop>,
) -> Result<Vec<ArtifactSearchResult>, String> {
    if crops.len() < 3 {
        return Err("At least three source crops are required".into());
    }
    let samples = crops[0].analysis.len();
    if samples == 0 || crops.iter().any(|crop| crop.analysis.len() != samples) {
        return Err("Source crops do not share a usable sample grid".into());
    }
    let mut baselines = vec![f32::NAN; samples];
    let mut values = Vec::with_capacity(crops.len());
    for (sample, baseline) in baselines.iter_mut().enumerate() {
        values.clear();
        values.extend(
            crops
                .iter()
                .map(|crop| crop.analysis[sample])
                .filter(|value| value.is_finite()),
        );
        if values.len() >= 2 {
            *baseline = median_in_place(&mut values);
        }
    }
    let mut residuals = crops
        .iter()
        .map(|crop| {
            crop.analysis
                .iter()
                .zip(&baselines)
                .map(|(value, baseline)| {
                    if value.is_finite() && baseline.is_finite() {
                        *value - *baseline
                    } else {
                        f32::NAN
                    }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let mut finite_residuals = residuals
        .iter()
        .flatten()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if finite_residuals.len() < 16 {
        return Err("The selected region has too little common frame coverage".into());
    }
    let center = median_in_place(&mut finite_residuals);
    for value in &mut finite_residuals {
        *value = (*value - center).abs();
    }
    let sigma = (median_in_place(&mut finite_residuals) * 1.4826).max(f32::EPSILON);

    let mut output = Vec::with_capacity(crops.len());
    for (crop, frame_residuals) in crops.into_iter().zip(residuals.iter_mut()) {
        let mut absolute_sigma = Vec::new();
        let mut bright = 0usize;
        let mut dark = 0usize;
        let mut finite = 0usize;
        for residual in frame_residuals
            .iter()
            .copied()
            .filter(|value| value.is_finite())
        {
            let z = (residual - center) / sigma;
            finite += 1;
            if z >= OUTLIER_SIGMA {
                bright += 1;
            } else if z <= -OUTLIER_SIGMA {
                dark += 1;
            }
            absolute_sigma.push(z.abs());
        }
        if finite == 0 {
            continue;
        }
        absolute_sigma.sort_unstable_by(f32::total_cmp);
        let peak_index = ((absolute_sigma.len() - 1) as f32 * 0.995).round() as usize;
        let peak_sigma = absolute_sigma[peak_index];
        let bright_fraction = bright as f32 / finite as f32;
        let dark_fraction = dark as f32 / finite as f32;
        let outlier_fraction = bright_fraction + dark_fraction;
        let score = peak_sigma + outlier_fraction * 50.0;
        let direction = if bright_fraction > dark_fraction * 1.5 {
            "bright"
        } else if dark_fraction > bright_fraction * 1.5 {
            "dark"
        } else {
            "mixed"
        };
        output.push(ArtifactSearchResult {
            image_id: crop.source.image_id,
            filter_name: crop.source.filter_name,
            acquired_unix_seconds: crop.source.acquired_unix_seconds,
            grading_status: crop.source.grading_status,
            score,
            peak_sigma,
            bright_fraction,
            dark_fraction,
            coverage_fraction: finite as f32 / samples as f32,
            evidence: "low".into(),
            direction: direction.into(),
            crop_url: format!(
                "/api/db/{database_id}/stack-previews/artifact-searches/{search_id}/crops/{}",
                crop.source.image_id
            ),
        });
    }
    for index in 0..output.len() {
        let mut peers = output
            .iter()
            .enumerate()
            .filter(|(peer_index, _)| *peer_index != index)
            .map(|(_, result)| result.score)
            .collect::<Vec<_>>();
        let peer_score = median_in_place(&mut peers);
        let excess = output[index].score - peer_score;
        let outlier_fraction = output[index].bright_fraction + output[index].dark_fraction;
        output[index].evidence = if output[index].peak_sigma >= 8.0
            && outlier_fraction >= 0.002
            && excess >= (peer_score * 0.5).max(2.0)
        {
            "strong".into()
        } else if output[index].peak_sigma >= 5.0
            && outlier_fraction >= 0.0005
            && excess >= (peer_score * 0.25).max(1.0)
        {
            "possible".into()
        } else {
            "low".into()
        };
    }
    output.sort_by(|left, right| right.score.total_cmp(&left.score));
    Ok(output)
}

fn median_in_place(values: &mut [f32]) -> f32 {
    values.sort_unstable_by(f32::total_cmp);
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        (values[middle - 1] + values[middle]) * 0.5
    } else {
        values[middle]
    }
}

fn validate_region(region: ReferenceRegion) -> Result<(), AppError> {
    if region.width < MIN_REGION_EDGE || region.height < MIN_REGION_EDGE {
        return Err(AppError::BadRequest(format!(
            "Artifact search regions must be at least {MIN_REGION_EDGE} × {MIN_REGION_EDGE} pixels"
        )));
    }
    if region.width > MAX_REGION_EDGE || region.height > MAX_REGION_EDGE {
        return Err(AppError::BadRequest(format!(
            "Artifact search regions are limited to {MAX_REGION_EDGE} × {MAX_REGION_EDGE} pixels"
        )));
    }
    Ok(())
}

fn validate_region_bounds(
    region: ReferenceRegion,
    width: usize,
    height: usize,
) -> Result<(), AppError> {
    let right = region
        .x
        .checked_add(region.width)
        .ok_or_else(|| AppError::BadRequest("Artifact search region overflows".into()))?;
    let bottom = region
        .y
        .checked_add(region.height)
        .ok_or_else(|| AppError::BadRequest("Artifact search region overflows".into()))?;
    if right > width || bottom > height {
        return Err(AppError::BadRequest(
            "Artifact search region lies outside the stack".into(),
        ));
    }
    Ok(())
}

fn validate_search_id(search_id: &str) -> Result<(), AppError> {
    if search_id.len() == 64 && search_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::BadRequest("Invalid artifact search ID".into()))
    }
}

fn hash_region(hasher: &mut Sha256, region: ReferenceRegion) {
    hasher.update(region.x.to_le_bytes());
    hasher.update(region.y.to_le_bytes());
    hasher.update(region.width.to_le_bytes());
    hasher.update(region.height.to_le_bytes());
}

fn hex_digest(digest: impl AsRef<[u8]>) -> String {
    let bytes = digest.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(&mut output, "{byte:02x}").expect("writing to a String cannot fail");
    }
    output
}

fn artifact_dir(cache_root: &FsPath, search_id: &str) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("artifact-searches")
        .join(search_id)
}

fn artifact_manifest_path(cache_root: &FsPath, search_id: &str) -> PathBuf {
    artifact_dir(cache_root, search_id).join("manifest.json")
}

fn artifact_crop_path(cache_root: &FsPath, search_id: &str, image_id: i32) -> PathBuf {
    artifact_dir(cache_root, search_id).join(format!("image-{image_id}.png"))
}

fn load_artifact_job(
    state: &Arc<AppState>,
    ctx: &DbContext,
    search_id: &str,
) -> Result<ArtifactSearchJob, AppError> {
    if let Some(job) = state.stack_previews.get_artifact_search(search_id) {
        return Ok(job);
    }
    let bytes = std::fs::read(artifact_manifest_path(&ctx.cache_dir_path, search_id))
        .map_err(|_| AppError::NotFound)?;
    let job = serde_json::from_slice::<ArtifactSearchJob>(&bytes).map_err(|error| {
        AppError::InternalError(format!("Invalid artifact search manifest: {error}"))
    })?;
    let _ = state.stack_previews.insert_artifact_search(job.clone());
    Ok(job)
}

fn persist_artifact_job(cache_root: &FsPath, job: &ArtifactSearchJob) -> Result<(), String> {
    let path = artifact_manifest_path(cache_root, &job.search_id);
    let parent = path
        .parent()
        .ok_or_else(|| "Artifact manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(job).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_stacking::LinearImage;

    fn crop(image_id: i32, values: Vec<f32>) -> LoadedCrop {
        LoadedCrop {
            source: SearchSource {
                image_id,
                filter_name: "Ha".into(),
                acquired_unix_seconds: Some(i64::from(image_id)),
                grading_status: 0,
                path: PathBuf::from(format!("{image_id}.fits")),
                transform: SimilarityTransform::IDENTITY,
                normalization: None,
                fingerprint: format!("{image_id}"),
            },
            analysis: values,
        }
    }

    #[test]
    fn artifact_score_ranks_one_bright_patch_first() {
        let normal = vec![0.0; 100];
        let mut artifact = normal.clone();
        artifact[40..50].fill(10.0);
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![crop(1, normal.clone()), crop(2, normal), crop(3, artifact)],
        )
        .unwrap();

        assert_eq!(results[0].image_id, 3);
        assert_eq!(results[0].direction, "bright");
        assert_eq!(results[0].evidence, "strong");
    }

    #[test]
    fn artifact_score_is_honest_when_frames_agree() {
        let values = (0..100).map(|value| value as f32).collect::<Vec<_>>();
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![
                crop(1, values.clone()),
                crop(2, values.clone()),
                crop(3, values),
            ],
        )
        .unwrap();

        assert!(results.iter().all(|result| result.evidence == "low"));
        assert!(results.iter().all(|result| result.score == 0.0));
    }

    #[test]
    fn equally_distinct_crops_are_ranked_without_claiming_a_suspect() {
        let mut first = vec![0.0; 300];
        let mut second = first.clone();
        let mut third = first.clone();
        first[0..3].fill(10.0);
        second[100..103].fill(10.0);
        third[200..203].fill(10.0);

        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![crop(1, first), crop(2, second), crop(3, third)],
        )
        .unwrap();

        assert!(results.iter().all(|result| result.evidence == "low"));
    }

    #[test]
    fn sampled_luminance_caps_analysis_work() {
        let image = LinearImage::new(512, 512, 1, vec![1.0; 512 * 512]).unwrap();
        assert!(sampled_luminance(&image).len() <= MAX_ANALYSIS_SAMPLES);
    }
}
