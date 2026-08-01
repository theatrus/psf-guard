//! Opt-in dust-residual correction derived from repeated detector-space evidence.

use super::{
    hex_digest, load_artifact_job, load_stack_job, prepare_search, source_fingerprint,
    validate_search_id, ArtifactMorphology, ArtifactSearchJob, ArtifactSearchRequest,
    ArtifactSearchState, PreparedSearch, SearchSource,
};
use crate::{
    calibration::AppliedCalibration,
    server::{
        api::ApiResponse,
        extract::DbContext,
        handlers::AppError,
        stack_preview::{stretch, StackGroupState, StackPreviewImageQuery, StackPreviewImageSize},
        state::AppState,
    },
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
    build_residual_flat_patch, FitsFrame, FrameDisposition, LinearImage, LiveStacker,
    NormalizationMode, ReferenceRegion, ResidualFlatDiagnostics, ResidualFlatOptions,
    SimilarityTransform, StackOptions, RESIDUAL_FLAT_ALGORITHM_VERSION,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    path::{Path as FsPath, PathBuf},
    sync::Arc,
};
use tokio_util::io::ReaderStream;

const RESIDUAL_FLAT_CACHE_VERSION: u32 = 1;
const MAXIMUM_USER_GAIN: f32 = 1.3;
const MAX_RESIDUAL_FLAT_SAMPLES: usize = 64;

#[derive(Debug, Clone, Deserialize)]
pub struct ResidualFlatRequest {
    pub image_id: i32,
    #[serde(default = "default_maximum_gain")]
    pub maximum_gain: f32,
}

fn default_maximum_gain() -> f32 {
    1.2
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResidualFlatState {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidualFlatJob {
    pub schema_version: u32,
    pub correction_id: String,
    pub database_id: String,
    pub search_id: String,
    pub source_job_id: String,
    pub source_image_id: i32,
    pub filter_name: String,
    pub state: ResidualFlatState,
    pub phase: String,
    pub total_work_units: usize,
    pub completed_work_units: usize,
    pub created_unix_seconds: i64,
    pub options: ResidualFlatOptions,
    pub detector_region: Option<ReferenceRegion>,
    pub sample_count: usize,
    pub dither_span_pixels: Option<f64>,
    pub required_dither_span_pixels: Option<f64>,
    pub diagnostics: Option<ResidualFlatDiagnostics>,
    pub accepted_frames: usize,
    pub rejected_frames: usize,
    pub calibration: Option<AppliedCalibration>,
    #[serde(default)]
    pub notes: Vec<String>,
    pub error: Option<String>,
}

struct CompletedResidualFlat {
    detector_region: ReferenceRegion,
    sample_count: usize,
    dither_span_pixels: f64,
    required_dither_span_pixels: f64,
    diagnostics: ResidualFlatDiagnostics,
    accepted_frames: usize,
    rejected_frames: usize,
    calibration: AppliedCalibration,
    notes: Vec<String>,
}

pub async fn start_residual_flat(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, search_id)): Path<(String, String)>,
    Json(request): Json<ResidualFlatRequest>,
) -> Result<Json<ApiResponse<ResidualFlatJob>>, AppError> {
    validate_search_id(&search_id)?;
    if !request.maximum_gain.is_finite()
        || !(1.0..=MAXIMUM_USER_GAIN).contains(&request.maximum_gain)
    {
        return Err(AppError::BadRequest(format!(
            "Maximum dust-correction gain must be between 1 and {MAXIMUM_USER_GAIN}"
        )));
    }
    let search = load_artifact_job(&state, &ctx, &search_id)?;
    if search.database_id != ctx.id || search.state != ArtifactSearchState::Completed {
        return Err(AppError::NotFound);
    }
    if search.source_kind != "mono" {
        return Err(AppError::BadRequest(
            "Build a dust correction from one mono channel stack before recomposing color".into(),
        ));
    }
    let result = search
        .results
        .iter()
        .find(|result| result.image_id == request.image_id)
        .ok_or(AppError::NotFound)?;
    if !residual_flat_candidate(
        result.evidence.as_str(),
        result.direction.as_str(),
        result.morphology,
    ) {
        return Err(AppError::BadRequest(
            "This source result lacks repeated dark ring or broad-shadow evidence".into(),
        ));
    }

    let options = ResidualFlatOptions {
        maximum_gain: request.maximum_gain,
        ..ResidualFlatOptions::default()
    };
    let correction_id = correction_id(&search, request.image_id, &options)?;
    if let Some(existing) = state.stack_previews.get_residual_flat(&correction_id) {
        let reusable = matches!(
            existing.state,
            ResidualFlatState::Queued | ResidualFlatState::Running
        ) || (existing.state == ResidualFlatState::Completed
            && residual_flat_artifacts_ready(&ctx.cache_dir_path, &correction_id));
        if reusable {
            return Ok(Json(ApiResponse::success(existing)));
        }
    }
    if let Ok(bytes) = std::fs::read(residual_flat_manifest_path(
        &ctx.cache_dir_path,
        &correction_id,
    )) && let Ok(existing) = serde_json::from_slice::<ResidualFlatJob>(&bytes)
        && existing.state == ResidualFlatState::Completed
        && residual_flat_artifacts_ready(&ctx.cache_dir_path, &correction_id)
    {
        let _ = state.stack_previews.insert_residual_flat(existing.clone());
        return Ok(Json(ApiResponse::success(existing)));
    }

    let job = ResidualFlatJob {
        schema_version: RESIDUAL_FLAT_CACHE_VERSION,
        correction_id,
        database_id: ctx.id.clone(),
        search_id,
        source_job_id: search.source_job_id,
        source_image_id: request.image_id,
        filter_name: result.filter_name.clone(),
        state: ResidualFlatState::Queued,
        phase: "Waiting for the stack processor".into(),
        total_work_units: 0,
        completed_work_units: 0,
        created_unix_seconds: chrono::Utc::now().timestamp(),
        options,
        detector_region: None,
        sample_count: 0,
        dither_span_pixels: None,
        required_dither_span_pixels: None,
        diagnostics: None,
        accepted_frames: 0,
        rejected_frames: 0,
        calibration: None,
        notes: vec![
            "This derived preview does not change source files, catalog grades, or the saved stack"
                .into(),
        ],
        error: None,
    };
    if !state.stack_previews.insert_residual_flat(job.clone()) {
        return Err(AppError::BadRequest(
            "Too many stack jobs are active; wait for one to finish".into(),
        ));
    }
    enqueue_residual_flat(state, job.clone());
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_residual_flat(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, correction_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<ResidualFlatJob>>, AppError> {
    validate_correction_id(&correction_id)?;
    let job = load_residual_flat_job(&state, &ctx, &correction_id)?;
    if job.database_id != ctx.id {
        return Err(AppError::NotFound);
    }
    Ok(Json(ApiResponse::success(job)))
}

pub async fn get_residual_flat_response(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, correction_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let job = completed_job(&state, &ctx, &correction_id)?;
    serve_file(
        residual_flat_response_preview_path(&ctx.cache_dir_path, &job.correction_id),
        "image/png",
        None,
    )
    .await
}

pub async fn get_residual_flat_preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, correction_id)): Path<(String, String)>,
    Query(query): Query<StackPreviewImageQuery>,
) -> Result<Response, AppError> {
    let job = completed_job(&state, &ctx, &correction_id)?;
    let path = match query.size {
        StackPreviewImageSize::Screen => {
            residual_flat_preview_path(&ctx.cache_dir_path, &job.correction_id)
        }
        StackPreviewImageSize::Original => {
            residual_flat_original_preview_path(&ctx.cache_dir_path, &job.correction_id)
        }
    };
    serve_file(path, "image/png", None).await
}

pub async fn download_residual_flat_fits(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, correction_id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let job = completed_job(&state, &ctx, &correction_id)?;
    let filename = format!(
        "psf-guard-dust-corrected-{}-{}.fits",
        job.filter_name
            .replace(|character: char| !character.is_ascii_alphanumeric(), "-"),
        &job.correction_id[..12]
    );
    serve_file(
        residual_flat_fits_path(&ctx.cache_dir_path, &job.correction_id),
        "application/fits",
        Some(filename),
    )
    .await
}

fn enqueue_residual_flat(state: Arc<AppState>, job: ResidualFlatJob) {
    let permit = Arc::clone(&state.stack_previews.permit);
    tokio::spawn(async move {
        let Ok(_permit) = permit.acquire_owned().await else {
            return;
        };
        let guard = state.begin_interactive_job();
        let state_for_job = Arc::clone(&state);
        let correction_id = job.correction_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _guard = guard;
            run_residual_flat(&state_for_job, &job)
        })
        .await;
        if let Err(error) = result {
            state
                .stack_previews
                .update_residual_flat(&correction_id, |entry| {
                    entry.state = ResidualFlatState::Failed;
                    entry.phase = "Dust-correction worker failed".into();
                    entry.error = Some(format!("Dust-correction worker panicked: {error}"));
                });
        }
    });
}

fn run_residual_flat(state: &Arc<AppState>, requested: &ResidualFlatJob) {
    let correction_id = requested.correction_id.clone();
    state
        .stack_previews
        .update_residual_flat(&correction_id, |job| {
            job.state = ResidualFlatState::Running;
            job.phase = "Restoring stack provenance".into();
        });
    let result = run_residual_flat_inner(state, requested);
    state
        .stack_previews
        .update_residual_flat(&correction_id, |job| match result {
            Ok(completed) => {
                job.state = ResidualFlatState::Completed;
                job.phase = "Dust-corrected preview ready".into();
                job.completed_work_units = job.total_work_units;
                job.detector_region = Some(completed.detector_region);
                job.sample_count = completed.sample_count;
                job.dither_span_pixels = Some(completed.dither_span_pixels);
                job.required_dither_span_pixels = Some(completed.required_dither_span_pixels);
                job.diagnostics = Some(completed.diagnostics);
                job.accepted_frames = completed.accepted_frames;
                job.rejected_frames = completed.rejected_frames;
                job.calibration = Some(completed.calibration);
                job.notes.extend(completed.notes);
                job.error = None;
            }
            Err(error) => {
                job.state = ResidualFlatState::Failed;
                job.phase = "Dust correction stopped".into();
                job.error = Some(error);
            }
        });
    if let Some(job) = state.stack_previews.get_residual_flat(&correction_id)
        && let Some(ctx) = state.get_database(&job.database_id)
        && let Err(error) = persist_residual_flat_job(&ctx.cache_dir_path, &job)
    {
        tracing::warn!("Failed to persist residual-flat job {correction_id}: {error}");
    }
}

fn run_residual_flat_inner(
    state: &Arc<AppState>,
    requested: &ResidualFlatJob,
) -> Result<CompletedResidualFlat, String> {
    let ctx = state
        .get_database(&requested.database_id)
        .ok_or_else(|| "The database is no longer configured".to_string())?;
    let db_ctx = DbContext(Arc::clone(&ctx));
    let search =
        load_artifact_job(state, &db_ctx, &requested.search_id).map_err(app_error_message)?;
    let prepared =
        restore_prepared_mono_search(state, &db_ctx, &search).map_err(app_error_message)?;
    let group = prepared
        .groups
        .iter()
        .find(|group| {
            group
                .sources
                .iter()
                .any(|source| source.image_id == requested.source_image_id)
        })
        .cloned()
        .ok_or_else(|| "The selected source frame left the stack input set".to_string())?;
    if group.sources.len() < requested.options.minimum_samples {
        return Err(format!(
            "{} has {} integrated frames; dust correction needs at least {}",
            group.label,
            group.sources.len(),
            requested.options.minimum_samples
        ));
    }
    let evidence_sources = residual_evidence_sources(&group.sources, requested.source_image_id);
    state
        .stack_previews
        .update_residual_flat(&requested.correction_id, |job| {
            job.total_work_units = evidence_sources.len() + group.sources.len() + 4;
            job.phase = "Resolving ordinary calibration".into();
        });

    let calibration_conn = rusqlite::Connection::open(&ctx.database_path)
        .map_err(|error| format!("Opening calibration catalog: {error}"))?;
    calibration_conn
        .busy_timeout(std::time::Duration::from_secs(60))
        .map_err(|error| format!("Configuring calibration catalog: {error}"))?;
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| format!("Indexing calibration folders: {error}"))?;
    let paths = group
        .sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    let (masters, applied) = crate::calibration::resolve_or_build_masters_for_group(
        &calibration_conn,
        &ctx.cache_dir_path,
        &paths,
        Some(&directory_tree),
    )
    .map_err(|error| error.to_string())?;
    if applied.fingerprint != group.expected_calibration_fingerprint {
        return Err("Ordinary calibration changed; rebuild the source stack first".into());
    }

    let selected = group
        .sources
        .iter()
        .find(|source| source.image_id == requested.source_image_id)
        .ok_or_else(|| "The selected source frame is missing".to_string())?;
    let mut selected_frame = FitsFrame::open(&selected.path).map_err(|error| error.to_string())?;
    masters
        .apply(
            &mut selected_frame.image,
            selected_frame.exposure_seconds,
            selected_frame.bayer,
        )
        .map_err(|error| error.to_string())?;
    let selected_frame = selected_frame
        .into_prepared()
        .map_err(|error| error.to_string())?;
    let detector_region = detector_region_for_source(
        selected,
        prepared.public.region,
        selected_frame.image.width,
        selected_frame.image.height,
    )?;

    let pixels = selected_frame.image.pixel_count();
    let estimate = (pixels as u64)
        .saturating_mul(selected_frame.image.channels as u64)
        .saturating_mul(super::super::STACK_BYTES_PER_OUTPUT_SAMPLE);
    let worker_policy = state.worker_policy();
    if let Some(available) = crate::concurrency::available_memory_bytes()
        && estimate > (available as f64 * worker_policy.memory_budget_fraction) as u64
    {
        return Err(format!(
            "Estimated corrected-stack memory {} MiB exceeds the configured available-memory budget",
            estimate / (1024 * 1024)
        ));
    }
    let budget = crate::concurrency::plan_workers(
        None,
        &worker_policy,
        crate::concurrency::Priority::Interactive,
        Some(pixels),
    );
    let pool = ThreadPoolBuilder::new()
        .num_threads(budget.workers)
        .thread_name(|index| format!("residual-flat-{index}"))
        .build()
        .map_err(|error| error.to_string())?;
    tracing::info!(
        "Residual-flat preview {}: {} worker(s) — {}",
        requested.correction_id,
        budget.workers,
        budget.rationale
    );

    pool.install(|| {
        let mut crops = Vec::with_capacity(evidence_sources.len());
        let mut detector_positions = Vec::with_capacity(evidence_sources.len());
        state
            .stack_previews
            .update_residual_flat(&requested.correction_id, |job| {
                job.detector_region = Some(detector_region);
                job.phase = "Collecting detector-aligned evidence".into();
            });
        for source in evidence_sources {
            if source_fingerprint(&source.path) != source.fingerprint {
                return Err(format!(
                    "Image {} changed after the source stack was built",
                    source.image_id
                ));
            }
            let mut frame = FitsFrame::open(&source.path).map_err(|error| error.to_string())?;
            masters
                .apply(&mut frame.image, frame.exposure_seconds, frame.bayer)
                .map_err(|error| error.to_string())?;
            let frame = frame.into_prepared().map_err(|error| error.to_string())?;
            if frame.image.width != selected_frame.image.width
                || frame.image.height != selected_frame.image.height
                || frame.image.channels != selected_frame.image.channels
            {
                return Err(format!(
                    "Image {} does not share the selected detector sampling",
                    source.image_id
                ));
            }
            crops.push(crop_image(&frame.image, detector_region)?);
            detector_positions.push(mapped_detector_center(source, detector_region));
            state
                .stack_previews
                .update_residual_flat(&requested.correction_id, |job| {
                    job.completed_work_units += 1
                });
        }

        let (dither_span, distinct_positions) = detector_motion(&detector_positions);
        let required_dither_span = required_motion(detector_region);
        if distinct_positions < 3 || dither_span < required_dither_span {
            return Err(format!(
                "The detector feature moved only {dither_span:.1}px across the stack; need {required_dither_span:.1}px and at least three dither positions to separate it from sky structure"
            ));
        }
        state
            .stack_previews
            .update_residual_flat(&requested.correction_id, |job| {
                job.sample_count = crops.len();
                job.dither_span_pixels = Some(dither_span);
                job.required_dither_span_pixels = Some(required_dither_span);
                job.phase = "Estimating repeated response".into();
                job.completed_work_units += 1;
            });
        let built = build_residual_flat_patch(&crops, &requested.options)
            .map_err(|error| error.to_string())?;

        let output_dir = residual_flat_dir(&ctx.cache_dir_path, &requested.correction_id);
        std::fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
        let response_fits =
            residual_flat_response_fits_path(&ctx.cache_dir_path, &requested.correction_id);
        let response_temp =
            response_fits.with_extension(format!("{}.tmp.fits", std::process::id()));
        seiza_stacking::write_linear_image_fits_f32(
            &response_temp,
            built.patch.response(),
            &[],
            &[],
        )
        .map_err(|error| error.to_string())?;
        std::fs::rename(&response_temp, &response_fits).map_err(|error| error.to_string())?;
        render_response_preview_atomic(
            built.patch.response(),
            &residual_flat_response_preview_path(&ctx.cache_dir_path, &requested.correction_id),
        )?;
        state
            .stack_previews
            .update_residual_flat(&requested.correction_id, |job| {
                job.diagnostics = Some(built.diagnostics.clone());
                job.phase = "Building corrected stack".into();
                job.completed_work_units += 1;
            });

        let options = StackOptions {
            normalization: NormalizationMode::Global,
            ..StackOptions::default()
        };
        let mut accepted_frames = 0usize;
        let mut rejected_frames = 0usize;
        let mut notes = Vec::new();
        if group.sources.len() > MAX_RESIDUAL_FLAT_SAMPLES {
            notes.push(format!(
                "Estimated the response from {} evenly spaced inputs out of {} integrated frames",
                crops.len(),
                group.sources.len()
            ));
        }
        let mut stacker = None;
        for source in &group.sources {
            if source_fingerprint(&source.path) != source.fingerprint {
                return Err(format!(
                    "Image {} changed after the source stack was built",
                    source.image_id
                ));
            }
            let mut frame = FitsFrame::open(&source.path).map_err(|error| error.to_string())?;
            masters
                .apply(&mut frame.image, frame.exposure_seconds, frame.bayer)
                .map_err(|error| error.to_string())?;
            let mut frame = frame.into_prepared().map_err(|error| error.to_string())?;
            built
                .patch
                .apply_at(&mut frame.image, detector_region.x, detector_region.y)
                .map_err(|error| error.to_string())?;
            match stacker.as_mut() {
                None => {
                    stacker = Some(
                        LiveStacker::from_prepared_frame(frame, options.clone())
                            .map_err(|error| error.to_string())?,
                    );
                    accepted_frames += 1;
                }
                Some(stacker) => match stacker
                    .push_linear(frame.image)
                    .map_err(|error| error.to_string())?
                {
                    FrameDisposition::Accepted(_) => accepted_frames += 1,
                    FrameDisposition::Rejected(reason) => {
                        rejected_frames += 1;
                        notes.push(format!(
                            "Corrected image {} was not integrated: {reason}",
                            source.image_id
                        ));
                    }
                },
            }
            state
                .stack_previews
                .update_residual_flat(&requested.correction_id, |job| {
                    job.accepted_frames = accepted_frames;
                    job.rejected_frames = rejected_frames;
                    job.completed_work_units += 1;
                });
        }
        if accepted_frames < 2 {
            return Err("Fewer than two corrected frames could be integrated".into());
        }
        let stacker =
            stacker.ok_or_else(|| "No corrected reference frame was prepared".to_string())?;
        let reference_headers = stacker.reference_headers().to_vec();
        let snapshot = stacker.into_snapshot().map_err(|error| error.to_string())?;
        let fits = residual_flat_fits_path(&ctx.cache_dir_path, &requested.correction_id);
        let fits_temp = fits.with_extension(format!("{}.tmp.fits", std::process::id()));
        seiza_stacking::write_fits_f32(&fits_temp, &snapshot, &reference_headers)
            .map_err(|error| error.to_string())?;
        std::fs::rename(&fits_temp, &fits).map_err(|error| error.to_string())?;
        state
            .stack_previews
            .update_residual_flat(&requested.correction_id, |job| {
                job.phase = "Rendering corrected preview".into();
                job.completed_work_units += 1;
            });
        stretch::render_image_previews_atomic(
            &snapshot.image,
            &stretch::default_linear_config(),
            stretch::StackStretchSourceTransfer::Linear,
            &residual_flat_preview_path(&ctx.cache_dir_path, &requested.correction_id),
            &residual_flat_original_preview_path(&ctx.cache_dir_path, &requested.correction_id),
        )?;
        state
            .stack_previews
            .update_residual_flat(&requested.correction_id, |job| {
                job.completed_work_units += 1
            });

        if applied.flat_master.is_some() {
            notes.push(
                "A matching flat was applied first; this patch corrects only the repeated residual"
                    .into(),
            );
        } else {
            notes.push("No matching master flat was available for these lights".into());
        }
        Ok(CompletedResidualFlat {
            detector_region,
            sample_count: crops.len(),
            dither_span_pixels: dither_span,
            required_dither_span_pixels: required_dither_span,
            diagnostics: built.diagnostics,
            accepted_frames,
            rejected_frames,
            calibration: applied,
            notes,
        })
    })
}

fn restore_prepared_mono_search(
    state: &Arc<AppState>,
    ctx: &DbContext,
    search: &ArtifactSearchJob,
) -> Result<PreparedSearch, AppError> {
    if search.source_kind != "mono" {
        return Err(AppError::BadRequest(
            "Dust correction requires one mono source stack".into(),
        ));
    }
    let request = ArtifactSearchRequest {
        artifact_revision: search.artifact_revision.clone(),
        region: search.region,
    };
    let group_index = search.group_index.ok_or(AppError::NotFound)?;
    let stack = load_stack_job(state, ctx, &search.source_job_id)?;
    if stack.database_id != ctx.id || stack.artifact_revision != search.artifact_revision {
        return Err(AppError::Conflict(
            "The source stack changed; run the region search again".into(),
        ));
    }
    let group = stack
        .groups
        .get(group_index)
        .filter(|group| group.index == group_index && group.state == StackGroupState::Ready)
        .cloned()
        .ok_or(AppError::NotFound)?;
    let prepared = prepare_search(
        &ctx.0,
        &search.source_job_id,
        &search.source_kind,
        search.group_index,
        &request,
        vec![(group, None, None)],
    )?;
    if prepared.public.search_id != search.search_id {
        return Err(AppError::Conflict(
            "The source-frame provenance changed; run the region search again".into(),
        ));
    }
    Ok(prepared)
}

fn residual_flat_candidate(
    evidence: &str,
    direction: &str,
    morphology: ArtifactMorphology,
) -> bool {
    matches!(evidence, "strong" | "possible")
        && direction == "dark"
        && matches!(
            morphology,
            ArtifactMorphology::Ring | ArtifactMorphology::BroadDark
        )
}

fn app_error_message(error: AppError) -> String {
    match error {
        AppError::NotFound => "A required stack artifact was not found".into(),
        AppError::NotFoundMessage(message)
        | AppError::DatabaseError(message)
        | AppError::BadRequest(message)
        | AppError::Conflict(message)
        | AppError::Forbidden(message)
        | AppError::InternalError(message) => message,
        AppError::NotImplemented => "This stack operation is not implemented".into(),
    }
}

fn correction_id(
    search: &ArtifactSearchJob,
    image_id: i32,
    options: &ResidualFlatOptions,
) -> Result<String, AppError> {
    let mut hasher = Sha256::new();
    hasher.update(search.search_id.as_bytes());
    hasher.update(image_id.to_le_bytes());
    hasher.update(RESIDUAL_FLAT_CACHE_VERSION.to_le_bytes());
    hasher.update(RESIDUAL_FLAT_ALGORITHM_VERSION.to_le_bytes());
    hasher.update(serde_json::to_vec(options).map_err(|error| {
        AppError::InternalError(format!("Failed to encode residual-flat options: {error}"))
    })?);
    Ok(hex_digest(hasher.finalize()))
}

fn detector_region_for_source(
    source: &SearchSource,
    output_region: ReferenceRegion,
    source_width: usize,
    source_height: usize,
) -> Result<ReferenceRegion, String> {
    let transform = source_to_output_transform(source);
    let right = output_region
        .x
        .checked_add(output_region.width)
        .ok_or_else(|| "Selected region overflows".to_string())?;
    let bottom = output_region
        .y
        .checked_add(output_region.height)
        .ok_or_else(|| "Selected region overflows".to_string())?;
    let corners = [
        (output_region.x as f64, output_region.y as f64),
        (right as f64, output_region.y as f64),
        (output_region.x as f64, bottom as f64),
        (right as f64, bottom as f64),
    ]
    .map(|(x, y)| transform.inverse_apply(x, y));
    let minimum_x = corners
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::INFINITY, f64::min);
    let maximum_x = corners
        .iter()
        .map(|(x, _)| *x)
        .fold(f64::NEG_INFINITY, f64::max);
    let minimum_y = corners
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::INFINITY, f64::min);
    let maximum_y = corners
        .iter()
        .map(|(_, y)| *y)
        .fold(f64::NEG_INFINITY, f64::max);
    if ![minimum_x, maximum_x, minimum_y, maximum_y]
        .into_iter()
        .all(f64::is_finite)
    {
        return Err("The selected region cannot be mapped to detector coordinates".into());
    }
    let padding =
        ((output_region.width.min(output_region.height) as f64 * 0.25).ceil() as usize).max(16);
    let x = (minimum_x.floor() as isize - padding as isize).max(0) as usize;
    let y = (minimum_y.floor() as isize - padding as isize).max(0) as usize;
    let right = (maximum_x.ceil() as usize)
        .saturating_add(padding)
        .min(source_width);
    let bottom = (maximum_y.ceil() as usize)
        .saturating_add(padding)
        .min(source_height);
    if right <= x + 4 || bottom <= y + 4 {
        return Err("The detector-space correction region is too small".into());
    }
    Ok(ReferenceRegion {
        x,
        y,
        width: right - x,
        height: bottom - y,
    })
}

fn source_to_output_transform(source: &SearchSource) -> SimilarityTransform {
    match source.output_transform {
        Some(output) => source.mapping.transform().then(output),
        None => source.mapping.transform(),
    }
}

fn mapped_detector_center(source: &SearchSource, region: ReferenceRegion) -> (f64, f64) {
    source_to_output_transform(source).apply(
        region.x as f64 + region.width as f64 * 0.5,
        region.y as f64 + region.height as f64 * 0.5,
    )
}

fn crop_image(image: &LinearImage, region: ReferenceRegion) -> Result<LinearImage, String> {
    let right = region
        .x
        .checked_add(region.width)
        .ok_or_else(|| "Detector crop overflows".to_string())?;
    let bottom = region
        .y
        .checked_add(region.height)
        .ok_or_else(|| "Detector crop overflows".to_string())?;
    if right > image.width || bottom > image.height {
        return Err("Detector crop lies outside a source frame".into());
    }
    let mut data = Vec::with_capacity(region.width * region.height * image.channels);
    for y in region.y..bottom {
        let start = (y * image.width + region.x) * image.channels;
        let length = region.width * image.channels;
        data.extend_from_slice(&image.data[start..start + length]);
    }
    LinearImage::new(region.width, region.height, image.channels, data)
        .map_err(|error| error.to_string())
}

fn render_response_preview_atomic(
    response: &LinearImage,
    destination: &FsPath,
) -> Result<(), String> {
    let minimum = response
        .data
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .min_by(f32::total_cmp)
        .ok_or_else(|| "Residual-flat response has no finite samples".to_string())?;
    let span = (1.0 - minimum).max(1.0e-6);
    let samples = response
        .data
        .iter()
        .map(|value| (((*value - minimum) / span).clamp(0.0, 1.0) * 255.0).round() as u8)
        .collect::<Vec<_>>();
    let image = if response.channels == 1 {
        image::GrayImage::from_raw(response.width as u32, response.height as u32, samples)
            .map(image::DynamicImage::ImageLuma8)
    } else {
        image::RgbImage::from_raw(response.width as u32, response.height as u32, samples)
            .map(image::DynamicImage::ImageRgb8)
    }
    .ok_or_else(|| "Residual-flat preview dimensions do not match its samples".to_string())?;
    let parent = destination
        .parent()
        .ok_or_else(|| "Residual-flat preview path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = destination.with_extension(format!("{}.tmp.png", std::process::id()));
    image.save(&temporary).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, destination).map_err(|error| error.to_string())
}

fn detector_motion(positions: &[(f64, f64)]) -> (f64, usize) {
    let mut xs = positions.iter().map(|(x, _)| *x).collect::<Vec<_>>();
    let mut ys = positions.iter().map(|(_, y)| *y).collect::<Vec<_>>();
    xs.sort_unstable_by(f64::total_cmp);
    ys.sort_unstable_by(f64::total_cmp);
    let low = positions.len() / 5;
    let high = positions.len().saturating_sub(1 + low);
    let span = if positions.is_empty() {
        0.0
    } else {
        (xs[high] - xs[low]).hypot(ys[high] - ys[low])
    };
    let distinct = positions
        .iter()
        .map(|(x, y)| ((x / 2.0).round() as i64, (y / 2.0).round() as i64))
        .collect::<HashSet<_>>()
        .len();
    (span, distinct)
}

fn residual_evidence_sources(
    sources: &[SearchSource],
    selected_image_id: i32,
) -> Vec<&SearchSource> {
    if sources.len() <= MAX_RESIDUAL_FLAT_SAMPLES {
        return sources.iter().collect();
    }
    let mut indices = (0..MAX_RESIDUAL_FLAT_SAMPLES)
        .map(|index| index * (sources.len() - 1) / (MAX_RESIDUAL_FLAT_SAMPLES - 1))
        .collect::<Vec<_>>();
    if let Some(selected_index) = sources
        .iter()
        .position(|source| source.image_id == selected_image_id)
        && !indices.contains(&selected_index)
    {
        let replacement = indices
            .iter()
            .enumerate()
            .min_by_key(|(_, index)| index.abs_diff(selected_index))
            .map(|(position, _)| position)
            .unwrap_or(indices.len() - 1);
        indices[replacement] = selected_index;
        indices.sort_unstable();
    }
    indices.into_iter().map(|index| &sources[index]).collect()
}

fn required_motion(region: ReferenceRegion) -> f64 {
    (region.width.min(region.height) as f64 * 0.08).clamp(6.0, 24.0)
}

fn validate_correction_id(correction_id: &str) -> Result<(), AppError> {
    if correction_id.len() == 64 && correction_id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AppError::BadRequest(
            "Invalid residual-flat correction ID".into(),
        ))
    }
}

fn completed_job(
    state: &Arc<AppState>,
    ctx: &DbContext,
    correction_id: &str,
) -> Result<ResidualFlatJob, AppError> {
    validate_correction_id(correction_id)?;
    let job = load_residual_flat_job(state, ctx, correction_id)?;
    if job.database_id != ctx.id || job.state != ResidualFlatState::Completed {
        return Err(AppError::NotFound);
    }
    Ok(job)
}

fn load_residual_flat_job(
    state: &Arc<AppState>,
    ctx: &DbContext,
    correction_id: &str,
) -> Result<ResidualFlatJob, AppError> {
    if let Some(job) = state.stack_previews.get_residual_flat(correction_id) {
        return Ok(job);
    }
    let bytes = std::fs::read(residual_flat_manifest_path(
        &ctx.cache_dir_path,
        correction_id,
    ))
    .map_err(|_| AppError::NotFound)?;
    let job = serde_json::from_slice::<ResidualFlatJob>(&bytes).map_err(|error| {
        AppError::InternalError(format!("Invalid residual-flat manifest: {error}"))
    })?;
    let _ = state.stack_previews.insert_residual_flat(job.clone());
    Ok(job)
}

async fn serve_file(
    path: PathBuf,
    content_type: &'static str,
    download_name: Option<String>,
) -> Result<Response, AppError> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let length = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(format!("Failed to stat derived stack: {error}")))?
        .len();
    let mut builder = Response::builder()
        .status(StatusCode::OK)
        .header(CONTENT_TYPE, content_type)
        .header(CONTENT_LENGTH, length)
        .header(CACHE_CONTROL, "private, max-age=31536000, immutable");
    if let Some(download_name) = download_name {
        builder = builder.header(
            CONTENT_DISPOSITION,
            format!("attachment; filename=\"{download_name}\""),
        );
    }
    builder
        .body(Body::from_stream(ReaderStream::new(file)))
        .map_err(|error| AppError::InternalError(format!("Failed to serve derived stack: {error}")))
}

fn residual_flat_dir(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    cache_root
        .join("stack-previews")
        .join("residual-flats")
        .join(correction_id)
}

fn residual_flat_manifest_path(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    residual_flat_dir(cache_root, correction_id).join("manifest.json")
}

fn residual_flat_response_fits_path(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    residual_flat_dir(cache_root, correction_id).join("response.fits")
}

fn residual_flat_response_preview_path(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    residual_flat_dir(cache_root, correction_id).join("response.png")
}

fn residual_flat_preview_path(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    residual_flat_dir(cache_root, correction_id).join("corrected.png")
}

fn residual_flat_original_preview_path(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    residual_flat_dir(cache_root, correction_id).join("corrected-original.png")
}

fn residual_flat_fits_path(cache_root: &FsPath, correction_id: &str) -> PathBuf {
    residual_flat_dir(cache_root, correction_id).join("corrected.fits")
}

fn residual_flat_artifacts_ready(cache_root: &FsPath, correction_id: &str) -> bool {
    [
        residual_flat_response_fits_path(cache_root, correction_id),
        residual_flat_response_preview_path(cache_root, correction_id),
        residual_flat_preview_path(cache_root, correction_id),
        residual_flat_original_preview_path(cache_root, correction_id),
        residual_flat_fits_path(cache_root, correction_id),
    ]
    .iter()
    .all(|path| path.is_file())
}

fn persist_residual_flat_job(cache_root: &FsPath, job: &ResidualFlatJob) -> Result<(), String> {
    let path = residual_flat_manifest_path(cache_root, &job.correction_id);
    let parent = path
        .parent()
        .ok_or_else(|| "Residual-flat manifest path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec_pretty(job).map_err(|error| error.to_string())?;
    std::fs::write(&temporary, bytes).map_err(|error| error.to_string())?;
    std::fs::rename(&temporary, path).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use seiza_stacking::{NormalizationMap, RegisteredFrameMapping};

    fn source(transform: SimilarityTransform) -> SearchSource {
        let image = LinearImage::new(100, 80, 1, vec![1.0; 8_000]).unwrap();
        SearchSource {
            image_id: 1,
            filter_name: "Ha".into(),
            acquired_unix_seconds: None,
            grading_status: 0,
            path: PathBuf::from("one.fits"),
            mapping: RegisteredFrameMapping::new(
                100,
                80,
                transform,
                NormalizationMap::identity(&image),
            )
            .unwrap(),
            output_transform: None,
            fingerprint: "one".into(),
        }
    }

    #[test]
    fn only_repeated_dark_ring_or_shadow_evidence_can_start() {
        assert!(residual_flat_candidate(
            "strong",
            "dark",
            ArtifactMorphology::Ring
        ));
        assert!(residual_flat_candidate(
            "possible",
            "dark",
            ArtifactMorphology::BroadDark
        ));
        assert!(!residual_flat_candidate(
            "low",
            "dark",
            ArtifactMorphology::Ring
        ));
        assert!(!residual_flat_candidate(
            "strong",
            "bright",
            ArtifactMorphology::Ring
        ));
        assert!(!residual_flat_candidate(
            "strong",
            "dark",
            ArtifactMorphology::Diffuse
        ));
    }

    #[test]
    fn output_selection_maps_back_to_a_padded_detector_region() {
        let source = source(SimilarityTransform {
            translation_x: 10.0,
            translation_y: -4.0,
            ..SimilarityTransform::IDENTITY
        });
        let region = detector_region_for_source(
            &source,
            ReferenceRegion {
                x: 30,
                y: 20,
                width: 20,
                height: 16,
            },
            100,
            80,
        )
        .unwrap();
        assert_eq!(region.x, 4);
        assert_eq!(region.y, 8);
        assert_eq!(region.width, 52);
        assert_eq!(region.height, 48);
    }

    #[test]
    fn motion_measure_ignores_one_extreme_mapping() {
        let positions = [
            (0.0, 0.0),
            (10.0, 0.0),
            (20.0, 0.0),
            (30.0, 0.0),
            (500.0, 0.0),
        ];
        let (span, distinct) = detector_motion(&positions);
        assert_eq!(span, 20.0);
        assert_eq!(distinct, 5);
    }

    #[test]
    fn crop_keeps_interleaved_channels_and_checks_bounds() {
        let image = LinearImage::new(3, 2, 3, (0..18).map(|value| value as f32).collect()).unwrap();
        let crop = crop_image(
            &image,
            ReferenceRegion {
                x: 1,
                y: 0,
                width: 2,
                height: 2,
            },
        )
        .unwrap();
        assert_eq!(
            crop.data,
            [3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 12.0, 13.0, 14.0, 15.0, 16.0, 17.0]
        );
        assert!(crop_image(
            &image,
            ReferenceRegion {
                x: 2,
                y: 1,
                width: 2,
                height: 1,
            }
        )
        .is_err());
    }

    #[test]
    fn response_preview_remains_visible_when_most_samples_are_neutral() {
        let mut samples = vec![1.0; 100];
        samples[44] = 0.9;
        let response = LinearImage::new(10, 10, 1, samples).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let destination = directory.path().join("response.png");
        render_response_preview_atomic(&response, &destination).unwrap();
        let rendered = image::open(destination).unwrap().into_luma8();
        assert_eq!(rendered.get_pixel(4, 4).0[0], 0);
        assert_eq!(rendered.get_pixel(0, 0).0[0], 255);
    }

    #[test]
    fn evidence_sampling_is_bounded_and_keeps_the_selected_source() {
        let sources = (0..100)
            .map(|image_id| {
                let mut source = source(SimilarityTransform::IDENTITY);
                source.image_id = image_id;
                source
            })
            .collect::<Vec<_>>();
        let sampled = residual_evidence_sources(&sources, 50);
        assert_eq!(sampled.len(), MAX_RESIDUAL_FLAT_SAMPLES);
        assert!(sampled.iter().any(|source| source.image_id == 50));
        assert_eq!(sampled.first().unwrap().image_id, 0);
        assert_eq!(sampled.last().unwrap().image_id, 99);
    }

    #[test]
    fn completed_cache_requires_every_derived_artifact() {
        let directory = tempfile::tempdir().unwrap();
        let correction_id = "a".repeat(64);
        let paths = [
            residual_flat_response_fits_path(directory.path(), &correction_id),
            residual_flat_response_preview_path(directory.path(), &correction_id),
            residual_flat_preview_path(directory.path(), &correction_id),
            residual_flat_original_preview_path(directory.path(), &correction_id),
            residual_flat_fits_path(directory.path(), &correction_id),
        ];
        std::fs::create_dir_all(paths[0].parent().unwrap()).unwrap();
        for path in &paths {
            std::fs::write(path, b"fixture").unwrap();
        }
        assert!(residual_flat_artifacts_ready(
            directory.path(),
            &correction_id
        ));
        std::fs::remove_file(&paths[3]).unwrap();
        assert!(!residual_flat_artifacts_ready(
            directory.path(),
            &correction_id
        ));
    }
}
