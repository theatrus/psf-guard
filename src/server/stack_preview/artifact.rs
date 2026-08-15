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
use seiza_imgproc::components::{largest_connected_component, BinaryComponent, Connectivity};
use seiza_stacking::{
    AffineTransform, CalibrationMasters, ReferenceRegion, RegisteredFrameMapping,
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

const ARTIFACT_SEARCH_CACHE_VERSION: u32 = 4;
const MIN_REGION_EDGE: usize = 8;
const MAX_REGION_EDGE: usize = 512;
const MAX_ANALYSIS_SAMPLES: usize = 65_536;
const MAX_TOTAL_ANALYSIS_SAMPLES: usize = 4_000_000;
const MAX_RESULTS_PER_GROUP: usize = 50;
const OUTLIER_SIGMA: f32 = 5.0;
const ROBUST_PEAK_QUANTILE: f32 = 0.9995;
const MIN_OUTLIER_SAMPLES: usize = 3;

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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactMorphology {
    Ring,
    BroadDark,
    Linear,
    Compact,
    Diffuse,
    #[default]
    Unclassified,
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
    #[serde(default)]
    pub morphology: ArtifactMorphology,
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
    pub total_work_units: usize,
    pub completed_work_units: usize,
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
    mapping: RegisteredFrameMapping,
    output_transform: Option<AffineTransform>,
    fingerprint: String,
}

#[derive(Clone)]
struct SearchGroup {
    label: String,
    expected_calibration_fingerprint: String,
    /// The masters the stack actually applied. Selection fingerprints are
    /// computed before builds, so with non-fatal builds two runs can share
    /// a fingerprint yet calibrate differently; crops must not silently
    /// compare against differently calibrated pixels. Empty for stacks
    /// recorded before this field existed — those skip the check.
    expected_masters_signature: String,
    /// The mode the stack calibrated under, so the search resolves the same
    /// way. Stacks recorded before the field existed were built in `Auto`.
    calibration_mode: crate::calibration::CalibrationMode,
    /// Per-session identity from the stack, when it recorded one. Lets the
    /// search validate session by session — a night whose every frame was
    /// rejected is absent here without invalidating the rest — and pins
    /// each session's fitted pedestal.
    expected_sessions: Vec<crate::calibration::CalibrationSessionDetail>,
    sources: Vec<SearchSource>,
}

type StackSearchGroup = (StackGroupStatus, Option<AffineTransform>, Option<String>);

struct PreparedSearch {
    public: ArtifactSearchJob,
    groups: Vec<SearchGroup>,
    reference_width: usize,
    reference_height: usize,
    cache_root: PathBuf,
}

struct LoadedCrop {
    source: SearchSource,
    analysis: SampleGrid,
}

struct SampleGrid {
    width: usize,
    height: usize,
    values: Vec<f32>,
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
    let output_transform = group
        .sky_orientation
        .as_ref()
        .filter(|orientation| orientation.is_current())
        .map(|orientation| orientation.source_to_output)
        .ok_or_else(|| {
            AppError::BadRequest(
                "Rebuild this stack before searching; its output mapping is missing".into(),
            )
        })?;
    let ctx_arc = Arc::clone(&ctx.0);
    let prepared = tokio::task::spawn_blocking(move || {
        prepare_search(
            &ctx_arc,
            &job_id,
            "mono",
            Some(group_index),
            &request,
            vec![(group, Some(output_transform), None)],
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Artifact preparation failed: {error}")))??;
    start_prepared_search(state, &ctx, prepared)
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
    start_prepared_search(state, &ctx, prepared)
}

pub async fn get_artifact_search(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, search_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<ArtifactSearchJob>>, AppError> {
    validate_search_id(&search_id)?;
    let mut job = load_artifact_job(&state, &ctx, &search_id)?;
    if job.database_id != ctx.id {
        return Err(AppError::NotFound);
    }
    refresh_result_grades(&ctx, &mut job)?;
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
    ctx: &DbContext,
    prepared: PreparedSearch,
) -> Result<Json<ApiResponse<ArtifactSearchJob>>, AppError> {
    if let Some(mut existing) = state
        .stack_previews
        .get_artifact_search(&prepared.public.search_id)
        && matches!(
            existing.state,
            ArtifactSearchState::Queued
                | ArtifactSearchState::Running
                | ArtifactSearchState::Completed
        )
    {
        refresh_result_grades(ctx, &mut existing)?;
        let _ = state
            .stack_previews
            .insert_artifact_search(existing.clone());
        return Ok(Json(ApiResponse::success(existing)));
    }
    if let Ok(bytes) = std::fs::read(artifact_manifest_path(
        &prepared.cache_root,
        &prepared.public.search_id,
    )) && let Ok(mut existing) = serde_json::from_slice::<ArtifactSearchJob>(&bytes)
        && existing.state == ArtifactSearchState::Completed
    {
        refresh_result_grades(ctx, &mut existing)?;
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

fn refresh_result_grades(ctx: &DbContext, job: &mut ArtifactSearchJob) -> Result<(), AppError> {
    if job.state != ArtifactSearchState::Completed || job.results.is_empty() {
        return Ok(());
    }
    let image_ids = job
        .results
        .iter()
        .map(|result| result.image_id)
        .collect::<Vec<_>>();
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let grades = Database::new(&conn)
        .get_images_by_ids(&image_ids)
        .map_err(AppError::db)?
        .into_iter()
        .map(|image| (image.id, image.grading_status))
        .collect::<HashMap<_, _>>();
    for result in &mut job.results {
        if let Some(grading_status) = grades.get(&result.image_id) {
            result.grading_status = *grading_status;
        }
    }
    Ok(())
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
) -> Result<Vec<StackSearchGroup>, AppError> {
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
            let stack_transform = group
                .sky_orientation
                .as_ref()
                .filter(|orientation| orientation.is_current())
                .map(|orientation| orientation.source_to_output)
                .ok_or_else(|| {
                    AppError::BadRequest(format!(
                        "Rebuild the {} stack before searching; its output mapping is missing",
                        source.role.label()
                    ))
                })?;
            Ok((
                group,
                Some(stack_transform.then(transform.as_affine())),
                Some(source.role.label().to_string()),
            ))
        })
        .collect()
}

fn prepare_search(
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    source_job_id: &str,
    source_kind: &str,
    group_index: Option<usize>,
    request: &ArtifactSearchRequest,
    stack_groups: Vec<StackSearchGroup>,
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
            let mapping = decision.registered_mapping.ok_or_else(|| {
                AppError::BadRequest(
                    "Rebuild this stack before searching; its frame mappings were not retained"
                        .into(),
                )
            })?;
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
                mapping,
                output_transform,
                fingerprint,
            };
            hasher.update(source.image_id.to_le_bytes());
            hasher.update(source.fingerprint.as_bytes());
            hasher.update(serde_json::to_vec(&source.mapping).map_err(|error| {
                AppError::InternalError(format!("Failed to encode frame mapping: {error}"))
            })?);
            hasher.update(
                serde_json::to_vec(&source.output_transform).map_err(|error| {
                    AppError::InternalError(format!("Failed to encode output transform: {error}"))
                })?,
            );
            sources.push(source);
        }
        groups.push(SearchGroup {
            label,
            expected_calibration_fingerprint: group.calibration.fingerprint,
            expected_masters_signature: group.calibration.masters_signature,
            calibration_mode: group.calibration.mode,
            expected_sessions: group.calibration.session_details,
            sources,
        });
    }
    if groups.is_empty() {
        return Err(AppError::BadRequest(notes.first().cloned().unwrap_or_else(
            || "No integrated source frames can be searched".into(),
        )));
    }
    let search_id = hex_digest(hasher.finalize());
    let total_work_units = groups
        .iter()
        .map(|group| group.sources.len() + group.sources.len().min(MAX_RESULTS_PER_GROUP))
        .sum();
    Ok(PreparedSearch {
        public: ArtifactSearchJob {
            schema_version: 2,
            search_id,
            database_id: ctx.id.clone(),
            source_job_id: source_job_id.into(),
            source_kind: source_kind.into(),
            group_index,
            artifact_revision: request.artifact_revision.clone(),
            region: request.region,
            state: ArtifactSearchState::Queued,
            phase: "Waiting for the stack processor".into(),
            total_work_units,
            completed_work_units: 0,
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
                job.completed_work_units = job.total_work_units;
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
        let render_during_scan = group.sources.len() <= MAX_RESULTS_PER_GROUP;
        state
            .stack_previews
            .update_artifact_search(&prepared.public.search_id, |job| {
                job.phase = if render_during_scan {
                    format!("Scanning and rendering {} source frames", group.label)
                } else {
                    format!("Scanning {} source frames", group.label)
                };
            });
        let paths = group
            .sources
            .iter()
            .map(|source| source.path.clone())
            .collect::<Vec<_>>();
        let plan = crate::calibration::resolve_or_build_master_plan(
            &calibration_conn,
            &prepared.cache_root,
            &paths,
            Some(&directory_tree),
            None,
            group.calibration_mode,
            &group.expected_sessions,
        )
        // `{:#}` keeps the anyhow cause chain; `to_string()` would report
        // only "building master <kind>" with no reason.
        .map_err(|error| format!("{error:#}"))?;
        if group.expected_sessions.is_empty() {
            // A stack recorded before per-session details: compare the
            // group-level identity exactly as it was recorded.
            if plan.applied.fingerprint != group.expected_calibration_fingerprint {
                return Err(format!(
                    "The calibration selected for {} changed; rebuild the stack before searching",
                    group.label
                ));
            }
            if !group.expected_masters_signature.is_empty()
                && plan.applied.masters_signature != group.expected_masters_signature
            {
                return Err(format!(
                    "The calibration masters for {} differ from the stack's (a master build failed or recovered since); rebuild the stack before searching",
                    group.label
                ));
            }
        } else {
            // Session-by-session: the searched sources may omit a whole
            // session (every frame rejected), but every session they DO
            // calibrate under must match what the stack recorded.
            for session in &plan.sessions {
                let expected = group
                    .expected_sessions
                    .iter()
                    .find(|detail| detail.fingerprint == session.applied.fingerprint);
                let Some(expected) = expected else {
                    return Err(format!(
                        "The calibration selected for {} changed; rebuild the stack before searching",
                        group.label
                    ));
                };
                if session.applied.masters_signature != expected.masters_signature {
                    return Err(format!(
                        "The calibration masters for {} differ from the stack's (a master build failed or recovered since); rebuild the stack before searching",
                        group.label
                    ));
                }
            }
        }
        let max_samples_per_frame =
            (MAX_TOTAL_ANALYSIS_SAMPLES / group.sources.len()).clamp(1, MAX_ANALYSIS_SAMPLES);
        let mut crops = Vec::with_capacity(group.sources.len());
        for (source_index, source) in group.sources.iter().enumerate() {
            if source_fingerprint(&source.path) != source.fingerprint {
                return Err(format!(
                    "Image {} changed while the source-frame search was waiting; rebuild the stack before searching",
                    source.image_id
                ));
            }
            let masters = &plan.sessions[plan.assignments[source_index]].masters;
            let crop = extract_source_crop(source, masters, prepared)?;
            let analysis = sampled_luminance(&crop, max_samples_per_frame);
            crops.push(LoadedCrop {
                source: source.clone(),
                analysis,
            });
            if render_during_scan {
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
            }
            state
                .stack_previews
                .update_artifact_search(&prepared.public.search_id, |job| {
                    job.completed_work_units += if render_during_scan { 2 } else { 1 }
                });
        }
        state
            .stack_previews
            .update_artifact_search(&prepared.public.search_id, |job| {
                job.phase = format!("Ranking {} source frames", group.label);
            });
        let mut ranked = score_group(
            &prepared.public.database_id,
            &prepared.public.search_id,
            crops,
        )?;
        if ranked.len() > MAX_RESULTS_PER_GROUP {
            state
                .stack_previews
                .update_artifact_search(&prepared.public.search_id, |job| {
                    job.notes.push(format!(
                        "{} shows the {MAX_RESULTS_PER_GROUP} strongest source matches out of {} frames",
                        group.label,
                        ranked.len()
                    ));
                });
            ranked.truncate(MAX_RESULTS_PER_GROUP);
        }
        if !render_during_scan {
            state
                .stack_previews
                .update_artifact_search(&prepared.public.search_id, |job| {
                    job.phase = format!("Rendering {} source crops", group.label);
                });
            for result in &ranked {
                let source = group
                    .sources
                    .iter()
                    .find(|source| source.image_id == result.image_id)
                    .ok_or_else(|| format!("Image {} left the source set", result.image_id))?;
                let source_index = group
                    .sources
                    .iter()
                    .position(|candidate| candidate.image_id == source.image_id)
                    .expect("ranked results come from group.sources");
                let masters = &plan.sessions[plan.assignments[source_index]].masters;
                let crop = extract_source_crop(source, masters, prepared)?;
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
                state
                    .stack_previews
                    .update_artifact_search(&prepared.public.search_id, |job| {
                        job.completed_work_units += 1
                    });
            }
        }
        all_results.extend(ranked);
    }
    all_results.sort_by(|left, right| {
        left.filter_name
            .cmp(&right.filter_name)
            .then_with(|| right.score.total_cmp(&left.score))
            .then_with(|| left.image_id.cmp(&right.image_id))
    });
    Ok(all_results)
}

fn extract_source_crop(
    source: &SearchSource,
    masters: &CalibrationMasters,
    prepared: &PreparedSearch,
) -> Result<seiza_stacking::LinearImage, String> {
    let mut frame =
        crate::image_io::open_linear_frame(&source.path).map_err(|error| error.to_string())?;
    masters
        .apply(&mut frame.image, frame.exposure_seconds, frame.bayer)
        .map_err(|error| error.to_string())?;
    let frame = frame.into_prepared().map_err(|error| error.to_string())?;
    match source.output_transform {
        Some(output_transform) => source
            .mapping
            .extract_region_after_affine(
                &frame.image,
                prepared.reference_width,
                prepared.reference_height,
                prepared.public.region,
                output_transform,
            )
            .map_err(|error| error.to_string()),
        None => source
            .mapping
            .extract_region(&frame.image, prepared.public.region)
            .map_err(|error| error.to_string()),
    }
}

fn sampled_luminance(image: &seiza_stacking::LinearImage, max_samples: usize) -> SampleGrid {
    let luminance = image.luminance();
    if luminance.len() <= max_samples {
        return SampleGrid {
            width: image.width,
            height: image.height,
            values: luminance,
        };
    }
    let aspect_ratio = image.width as f64 / image.height as f64;
    let columns =
        ((max_samples as f64 * aspect_ratio).sqrt().floor() as usize).clamp(1, image.width);
    let rows = (max_samples / columns).clamp(1, image.height);
    let mut sampled = Vec::with_capacity(columns * rows);
    for row in 0..rows {
        let top = row * image.height / rows;
        let bottom = (row + 1) * image.height / rows;
        for column in 0..columns {
            let left = column * image.width / columns;
            let right = (column + 1) * image.width / columns;
            let mut sum = 0.0_f64;
            let mut finite = 0usize;
            for y in top..bottom {
                for value in &luminance[y * image.width + left..y * image.width + right] {
                    if value.is_finite() {
                        sum += f64::from(*value);
                        finite += 1;
                    }
                }
            }
            sampled.push(if finite == 0 {
                f32::NAN
            } else {
                (sum / finite as f64) as f32
            });
        }
    }
    SampleGrid {
        width: columns,
        height: rows,
        values: sampled,
    }
}

fn score_group(
    database_id: &str,
    search_id: &str,
    mut crops: Vec<LoadedCrop>,
) -> Result<Vec<ArtifactSearchResult>, String> {
    if crops.len() < 3 {
        return Err("At least three source crops are required".into());
    }
    let sample_width = crops[0].analysis.width;
    let sample_height = crops[0].analysis.height;
    let samples = crops[0].analysis.values.len();
    if samples == 0
        || sample_width.saturating_mul(sample_height) != samples
        || crops.iter().any(|crop| {
            crop.analysis.width != sample_width
                || crop.analysis.height != sample_height
                || crop.analysis.values.len() != samples
        })
    {
        return Err("Source crops do not share a usable sample grid".into());
    }
    let mut baselines = vec![f32::NAN; samples];
    let mut values = Vec::with_capacity(crops.len());
    for (sample, baseline) in baselines.iter_mut().enumerate() {
        values.clear();
        values.extend(
            crops
                .iter()
                .map(|crop| crop.analysis.values[sample])
                .filter(|value| value.is_finite()),
        );
        if values.len() >= 2 {
            *baseline = median_in_place(&mut values);
        }
    }
    for crop in &mut crops {
        for (value, baseline) in crop.analysis.values.iter_mut().zip(&baselines) {
            *value = if value.is_finite() && baseline.is_finite() {
                *value - *baseline
            } else {
                f32::NAN
            };
        }
    }
    let mut finite_residuals = crops
        .iter()
        .flat_map(|crop| &crop.analysis.values)
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
    for crop in crops {
        let mut absolute_sigma = Vec::new();
        let mut sigma_values = Vec::with_capacity(samples);
        let mut bright = 0usize;
        let mut dark = 0usize;
        let mut finite = 0usize;
        for residual in crop.analysis.values.iter().copied() {
            let z = if residual.is_finite() {
                (residual - center) / sigma
            } else {
                f32::NAN
            };
            sigma_values.push(z);
            if !z.is_finite() {
                continue;
            }
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
        let peak_index =
            ((absolute_sigma.len() - 1) as f32 * ROBUST_PEAK_QUANTILE).round() as usize;
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
        let morphology =
            classify_morphology(&sigma_values, sample_width, sample_height, bright, dark);
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
            morphology,
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
        let outlier_samples = (outlier_fraction * samples as f32).round() as usize;
        output[index].evidence = if output[index].peak_sigma >= 8.0
            && outlier_fraction >= 0.002
            && outlier_samples >= MIN_OUTLIER_SAMPLES
            && excess >= (peer_score * 0.5).max(2.0)
        {
            "strong".into()
        } else if output[index].peak_sigma >= 5.0
            && outlier_fraction >= 0.0005
            && outlier_samples >= MIN_OUTLIER_SAMPLES
            && excess >= (peer_score * 0.25).max(1.0)
        {
            "possible".into()
        } else {
            "low".into()
        };
        if output[index].evidence == "low" {
            output[index].morphology = ArtifactMorphology::Unclassified;
        }
    }
    output.sort_by(|left, right| right.score.total_cmp(&left.score));
    Ok(output)
}

fn classify_morphology(
    sigma_values: &[f32],
    width: usize,
    height: usize,
    bright_pixels: usize,
    dark_pixels: usize,
) -> ArtifactMorphology {
    let bright = largest_threshold_component(sigma_values, width, height, true);
    let dark = largest_threshold_component(sigma_values, width, height, false);

    if bright
        .as_ref()
        .is_some_and(|component| is_ring(component, width))
        || dark
            .as_ref()
            .is_some_and(|component| is_ring(component, width))
    {
        return ArtifactMorphology::Ring;
    }

    if let Some(component) = dark.as_ref()
        && component.pixels.len() >= (sigma_values.len() / 200).max(12)
        && component.width().min(component.height()) >= 5
        && component.elongation <= 2.5
        && component.fill_fraction() >= 0.35
        && component.pixels.len() * 2 >= dark_pixels
    {
        return ArtifactMorphology::BroadDark;
    }

    let dominant = match (bright.as_ref(), dark.as_ref()) {
        (Some(bright), Some(dark)) => {
            if bright.pixels.len() >= dark.pixels.len() {
                Some(bright)
            } else {
                Some(dark)
            }
        }
        (Some(bright), None) => Some(bright),
        (None, Some(dark)) => Some(dark),
        (None, None) => None,
    };
    let Some(component) = dominant else {
        return ArtifactMorphology::Unclassified;
    };
    if component.pixels.len() >= 6
        && component.width().max(component.height()) >= 6
        && component.elongation >= 4.0
    {
        return ArtifactMorphology::Linear;
    }
    let active_pixels = bright_pixels + dark_pixels;
    if component.pixels.len() <= (sigma_values.len() / 500).max(12)
        && component.width().max(component.height()) <= 8
        && component.pixels.len() * 2 >= active_pixels
    {
        return ArtifactMorphology::Compact;
    }
    ArtifactMorphology::Diffuse
}

fn largest_threshold_component(
    sigma_values: &[f32],
    width: usize,
    height: usize,
    bright: bool,
) -> Option<BinaryComponent> {
    let mask = sigma_values
        .iter()
        .map(|value| {
            u8::from(if bright {
                *value >= OUTLIER_SIGMA
            } else {
                *value <= -OUTLIER_SIGMA
            })
        })
        .collect::<Vec<_>>();
    largest_connected_component(&mask, width, height, Connectivity::Eight)
}

fn is_ring(component: &BinaryComponent, image_width: usize) -> bool {
    if component.pixels.len() < 12
        || component.width().min(component.height()) < 5
        || component.elongation > 1.8
        || component.fill_fraction() > 0.68
    {
        return false;
    }
    let center_x = (component.min_x + component.max_x) as f32 * 0.5;
    let center_y = (component.min_y + component.max_y) as f32 * 0.5;
    let radius_x = (component.width() as f32 * 0.5).max(1.0);
    let radius_y = (component.height() as f32 * 0.5).max(1.0);
    let mut inner = 0usize;
    let mut annulus = 0usize;
    for &index in &component.pixels {
        let x = index % image_width;
        let y = index / image_width;
        let dx = (x as f32 - center_x) / radius_x;
        let dy = (y as f32 - center_y) / radius_y;
        let radius = (dx * dx + dy * dy).sqrt();
        if radius <= 0.35 {
            inner += 1;
        }
        if (0.45..=1.15).contains(&radius) {
            annulus += 1;
        }
    }
    inner as f32 / component.pixels.len() as f32 <= 0.05
        && annulus as f32 / component.pixels.len() as f32 >= 0.70
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
        crop_grid(image_id, 1, values.len(), values)
    }

    fn crop_grid(image_id: i32, width: usize, height: usize, values: Vec<f32>) -> LoadedCrop {
        assert_eq!(values.len(), width * height);
        LoadedCrop {
            source: SearchSource {
                image_id,
                filter_name: "Ha".into(),
                acquired_unix_seconds: Some(i64::from(image_id)),
                grading_status: 0,
                path: PathBuf::from(format!("{image_id}.fits")),
                mapping: RegisteredFrameMapping::identity(
                    &LinearImage::new(width, height, 1, values.clone()).unwrap(),
                ),
                output_transform: None,
                fingerprint: format!("{image_id}"),
            },
            analysis: SampleGrid {
                width,
                height,
                values,
            },
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
        assert!(results
            .iter()
            .all(|result| result.morphology == ArtifactMorphology::Unclassified));
    }

    #[test]
    fn artifact_score_recognizes_a_broad_dark_shadow() {
        let width = 64;
        let height = 64;
        let normal = vec![0.0; width * height];
        let mut artifact = normal.clone();
        for y in 0..height {
            for x in 0..width {
                let dx = x as isize - 32;
                let dy = y as isize - 32;
                if dx * dx + dy * dy <= 10 * 10 {
                    artifact[y * width + x] = -10.0;
                }
            }
        }
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![
                crop_grid(1, width, height, normal.clone()),
                crop_grid(2, width, height, normal),
                crop_grid(3, width, height, artifact),
            ],
        )
        .unwrap();

        assert_eq!(results[0].image_id, 3);
        assert_eq!(results[0].direction, "dark");
        assert_eq!(results[0].morphology, ArtifactMorphology::BroadDark);
    }

    #[test]
    fn morphology_recognizes_a_soft_dust_shadow() {
        let width = 64;
        let height = 64;
        let mut sigma_values = vec![0.0; width * height];
        let mut dark_pixels = 0;
        for y in 0..height {
            for x in 0..width {
                let dx = x as f32 - 32.0;
                let dy = y as f32 - 32.0;
                let z = -12.0 * (-(dx * dx + dy * dy) / (2.0 * 8.0_f32.powi(2))).exp();
                sigma_values[y * width + x] = z;
                if z <= -OUTLIER_SIGMA {
                    dark_pixels += 1;
                }
            }
        }

        assert_eq!(
            classify_morphology(&sigma_values, width, height, 0, dark_pixels),
            ArtifactMorphology::BroadDark
        );
    }

    #[test]
    fn artifact_score_recognizes_a_defocused_ring_shape() {
        let width = 64;
        let height = 64;
        let normal = vec![0.0; width * height];
        let mut artifact = normal.clone();
        for y in 0..height {
            for x in 0..width {
                let dx = x as isize - 32;
                let dy = y as isize - 32;
                let radius_squared = dx * dx + dy * dy;
                if (8 * 8..=12 * 12).contains(&radius_squared) {
                    artifact[y * width + x] = 10.0;
                }
            }
        }
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![
                crop_grid(1, width, height, normal.clone()),
                crop_grid(2, width, height, normal),
                crop_grid(3, width, height, artifact),
            ],
        )
        .unwrap();

        assert_eq!(results[0].image_id, 3);
        assert_eq!(results[0].morphology, ArtifactMorphology::Ring);
    }

    #[test]
    fn artifact_score_recognizes_a_diagonal_trail() {
        let width = 64;
        let height = 64;
        let normal = vec![0.0; width * height];
        let mut artifact = normal.clone();
        for position in 12..52 {
            artifact[position * width + position] = 10.0;
        }
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![
                crop_grid(1, width, height, normal.clone()),
                crop_grid(2, width, height, normal),
                crop_grid(3, width, height, artifact),
            ],
        )
        .unwrap();

        assert_eq!(results[0].image_id, 3);
        assert_eq!(results[0].morphology, ArtifactMorphology::Linear);
    }

    #[test]
    fn artifact_score_recognizes_a_compact_spot() {
        let width = 64;
        let height = 64;
        let normal = vec![0.0; width * height];
        let mut artifact = normal.clone();
        for y in 31..=32 {
            for x in 31..=32 {
                artifact[y * width + x] = 10.0;
            }
        }
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![
                crop_grid(1, width, height, normal.clone()),
                crop_grid(2, width, height, normal),
                crop_grid(3, width, height, artifact),
            ],
        )
        .unwrap();

        assert_eq!(results[0].image_id, 3);
        assert_eq!(results[0].morphology, ArtifactMorphology::Compact);
    }

    #[test]
    fn artifact_score_does_not_promote_one_hot_sample() {
        let width = 8;
        let height = 8;
        let normal = vec![0.0; width * height];
        let mut artifact = normal.clone();
        artifact[4 * width + 4] = 10.0;
        let results = score_group(
            "db",
            &"a".repeat(64),
            vec![
                crop_grid(1, width, height, normal.clone()),
                crop_grid(2, width, height, normal),
                crop_grid(3, width, height, artifact),
            ],
        )
        .unwrap();

        assert_eq!(results[0].evidence, "low");
        assert_eq!(results[0].morphology, ArtifactMorphology::Unclassified);
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
        assert!(
            sampled_luminance(&image, MAX_ANALYSIS_SAMPLES).values.len() <= MAX_ANALYSIS_SAMPLES
        );
    }

    #[test]
    fn sampled_luminance_does_not_skip_thin_vertical_features() {
        let mut values = vec![0.0; 512 * 512];
        for y in 0..512 {
            values[y * 512 + 2] = 100.0;
        }
        let image = LinearImage::new(512, 512, 1, values).unwrap();
        let sampled = sampled_luminance(&image, MAX_ANALYSIS_SAMPLES);

        assert!(sampled.values.iter().any(|value| *value > 0.0));
    }
}
