pub use annotated::{
    get_annotated_image, get_psf_visualization, post_generation_status, GenStatusItem,
    GenerationStatusBatch, GenerationStatusRequest, PsfMultiOptions,
};
use anyhow::Context as _;
use axum::{
    extract::{Extension, Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE, ETAG, IF_NONE_MATCH},
        HeaderMap, HeaderValue, StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
pub(crate) use export_routes::run_server_export;
pub use export_routes::{
    export_archive_route, export_local_route, get_server_export_progress,
    start_server_export_route, stop_server_export_route, CalibrationReportQuery, NightsQuery,
};
pub(crate) use image_files::find_fits_file_async;
use image_files::{
    canonical_image_roots, find_fits_file_cached_async, find_fits_file_cached_with_roots,
};
pub use image_files::{find_fits_file, get_image_preview, get_image_stars};
#[cfg(test)]
use image_files::{
    find_fits_file_cached, mapped_file_sha256, metadata_suffix_candidates,
    resolve_mapped_remote_image_with,
};
#[cfg(test)]
use image_routes::image_file_identity_cache_token;
pub(crate) use image_routes::{
    annotated_cache_key, artifact_cache_path, generating_response, preview_cache_key,
    rendered_artifact_mapping_revision, resolve_copy, resolve_image_meta, serve_cached_png,
    with_copy, ResolvedCopy,
};
use image_routes::{
    apply_grades, psf_multi_cache_key, resolve_copy_async, source_file_cache_token,
    star_detection_cache_key, tracked_cache_matches_source, write_cache_atomic,
};
pub use image_routes::{batch_update_image_grades, get_image, get_images, update_image_grade};
use import_routes::{fill_missing_star_metadata, sanitize_file_stem, unique_db_file};
pub use import_routes::{
    get_autoimport_status, get_import_folders, get_import_progress, run_autoimport_now,
    start_import_route, stop_import_route, ImportFolder,
};
pub(crate) use import_routes::{
    run_import, spawn_import_job, spawn_import_job_with_trigger, spawn_quality_backfill,
};
pub use overview::{get_overall_stats, get_projects_overview, get_targets_overview};
#[cfg(test)]
use quality_scan_routes::quality_scan_source;
pub(crate) use quality_scan_routes::{
    filename_from_metadata, merge_astrometry_metrics, merge_photometric_signals,
    merge_spatial_metrics, rotator_position_from_metadata, scan_target, stored_entry_for,
};
pub use quality_scan_routes::{get_spatial_scan_progress, start_spatial_scan};
pub use sequence::{analyze_sequence, get_image_quality};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Read as _;
use std::path::{Path as FsPath, PathBuf};
use std::sync::Arc;
pub use sync_routes::{
    apply_sync_database_preview_route, delete_sync_database_preview_route,
    get_sync_database_preview_route, list_sync_database_previews_route,
    preview_sync_database_route, refresh_sync_database_preview_route, sync_database_route,
};
pub(crate) use sync_routes::{execute_scheduler_sync_paths, SyncGuardMode};
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::db::Database;
use crate::models::{GradingStatus, OverallDesiredStats, ProjectDesiredStats};
use crate::server::api::*;
use crate::server::database_context::{
    error_has_sqlite_failure, error_is_sqlite_contention, open_scheduler_connection_with_flags,
    remote_image_file_identity, DatabaseContext,
};
use crate::server::extract::DbContext;
use crate::server::state::AppState;

// Helper function to format RA/Dec coordinates
fn format_coordinates(ra: Option<f64>, dec: Option<f64>) -> Option<String> {
    match (ra, dec) {
        (Some(ra_hours), Some(dec_deg)) => {
            // Target Scheduler stores RA in decimal hours and Dec in degrees.
            let ra_h = ra_hours.floor();
            let ra_m = ((ra_hours - ra_h) * 60.0).floor();
            let ra_s = ((ra_hours - ra_h) * 60.0 - ra_m) * 60.0;

            let dec_sign = if dec_deg >= 0.0 { "+" } else { "-" };
            let dec_abs = dec_deg.abs();
            let dec_d = dec_abs.floor();
            let dec_m = ((dec_abs - dec_d) * 60.0).floor();
            let dec_s = ((dec_abs - dec_d) * 60.0 - dec_m) * 60.0;

            Some(format!(
                "RA {:02.0}h {:02.0}m {:04.1}s, Dec {}{:02.0}° {:02.0}' {:04.1}\"",
                ra_h, ra_m, ra_s, dec_sign, dec_d, dec_m, dec_s
            ))
        }
        _ => None,
    }
}

mod annotated;
#[cfg(test)]
mod coordinate_format_tests;
mod export_routes;
mod image_files;
mod image_routes;
mod import_routes;
mod overview;
mod quality_scan_routes;
mod sequence;
mod sync_routes;

pub async fn get_server_info(
    State(state): State<Arc<AppState>>,
    access: Option<Extension<crate::server::auth::RequestAccess>>,
) -> Result<Json<ApiResponse<ServerInfo>>, AppError> {
    let access =
        access
            .map(|Extension(access)| access)
            .unwrap_or(crate::server::auth::RequestAccess {
                role: crate::server::auth::AccessRole::ReadWrite,
                username: None,
                api_token: false,
            });
    let info = ServerInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        cache_directory: state.storage_roots.cache.display().to_string(),
        allow_database_management: state.database_management_allowed()
            && access.role == crate::server::auth::AccessRole::ReadWrite,
        banner: state.site_banner(),
    };

    Ok(Json(ApiResponse::success(info)))
}

pub async fn get_update_notice(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<crate::server::update_notice::UpdateNoticeStatus>>, AppError> {
    state.update_notices.refresh_in_background_if_stale();
    Ok(Json(ApiResponse::success(state.update_notices.snapshot())))
}

/// Summarize PSF Guard's calibration library in this scheduler database.
/// Reading an untouched database does not create any sibling tables.
pub async fn get_calibration_library(
    ctx: DbContext,
) -> Result<Json<ApiResponse<crate::calibration::CalibrationLibrarySummary>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let summary = crate::calibration::library_summary(&conn).map_err(AppError::db)?;
    Ok(Json(ApiResponse::success(summary)))
}

pub async fn get_calibration_library_details(
    ctx: DbContext,
) -> Result<Json<ApiResponse<crate::calibration::CalibrationLibraryDetails>>, AppError> {
    let details = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        crate::calibration::library_details(&conn).map_err(AppError::db)?
    };
    // A frame outside the database's folders reads as missing; see
    // `ReadableRoots`.
    let roots = crate::server::calibration_browser::ReadableRoots::of(&ctx);
    let details = tokio::task::spawn_blocking(move || {
        let mut details = details;
        for frame in &mut details.frames {
            frame.source_exists = roots.holds(std::path::Path::new(&frame.source_path));
        }
        details
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("calibration source check failed: {error}"))
    })?;
    Ok(Json(ApiResponse::success(details)))
}

pub async fn forget_calibration_frame(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, frame_uuid)): Path<(String, String)>,
) -> Result<Json<ApiResponse<crate::calibration::CalibrationMutationOutcome>>, AppError> {
    let conn = ctx.db();
    let mut conn = conn.lock().map_err(AppError::db)?;
    // Its rig, read before the row goes: its flat session is measured
    // again, from the flats left.
    let rigs =
        crate::server::flat_measure::rigs_of_frames(&conn, std::slice::from_ref(&frame_uuid));
    let outcome = crate::calibration::forget_frame(&mut conn, &frame_uuid).map_err(AppError::db)?;
    // A forgotten frame may have been a peer the others were judged by.
    crate::server::calibration_quality::request(&state, &ctx.database_path);
    crate::server::flat_measure::queue(
        &state,
        &ctx.database_path,
        crate::calibration::flat_groups::job::Scope::flats(rigs),
    );
    if outcome.frames_removed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(ApiResponse::success(outcome)))
}

#[derive(Debug, serde::Deserialize)]
pub struct ForgetCalibrationFramesRequest {
    pub frame_uuids: Vec<String>,
}

/// DELETE /api/db/{db}/calibrations/frames — forget several catalog records
/// (a whole night of a kind) in one transaction. Files are never touched.
pub async fn forget_calibration_frames(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<ForgetCalibrationFramesRequest>,
) -> Result<Json<ApiResponse<crate::calibration::CalibrationMutationOutcome>>, AppError> {
    if request.frame_uuids.is_empty() {
        return Err(AppError::BadRequest("no frames given".into()));
    }
    let conn = ctx.db();
    let mut conn = conn.lock().map_err(AppError::db)?;
    // Their rigs, read before the rows go: their flat sessions are
    // measured again, from the flats left.
    let rigs = crate::server::flat_measure::rigs_of_frames(&conn, &request.frame_uuids);
    let outcome =
        crate::calibration::forget_frames(&mut conn, &request.frame_uuids).map_err(AppError::db)?;
    // A forgotten frame may have been a peer the others were judged by.
    crate::server::calibration_quality::request(&state, &ctx.database_path);
    crate::server::flat_measure::queue(
        &state,
        &ctx.database_path,
        crate::calibration::flat_groups::job::Scope::flats(rigs),
    );
    if outcome.frames_removed == 0 {
        return Err(AppError::NotFound);
    }
    Ok(Json(ApiResponse::success(outcome)))
}

#[derive(Debug, serde::Deserialize)]
pub struct SetCalibrationValidityRequest {
    pub frame_uuids: Vec<String>,
    /// "forward", "backward", or "both" (which clears the mark).
    pub direction: String,
}

#[derive(Debug, serde::Serialize)]
pub struct SetCalibrationValidityResponse {
    pub updated: usize,
}

/// PUT /api/db/{db}/calibrations/frames/validity — mark frames as usable
/// only for lights after them, only before them, or both again.
pub async fn set_calibration_validity(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<SetCalibrationValidityRequest>,
) -> Result<Json<ApiResponse<SetCalibrationValidityResponse>>, AppError> {
    let direction = match request.direction.as_str() {
        "both" => None,
        "forward" => Some(crate::calibration::ValidDirection::Forward),
        "backward" => Some(crate::calibration::ValidDirection::Backward),
        other => {
            return Err(AppError::BadRequest(format!(
                "direction must be both, forward, or backward, not {other:?}"
            )));
        }
    };
    if request.frame_uuids.is_empty() {
        return Err(AppError::BadRequest("no frames given".into()));
    }
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let updated = crate::calibration::set_frames_validity(&conn, &request.frame_uuids, direction)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    if updated == 0 {
        return Err(AppError::NotFound);
    }
    // What each project's lights get has changed.
    crate::server::calibration_health::request(&state, &ctx.database_path);
    Ok(Json(ApiResponse::success(SetCalibrationValidityResponse {
        updated,
    })))
}

#[derive(Debug, serde::Deserialize)]
pub struct SetCalibrationQualityOverrideRequest {
    pub frame_uuids: Vec<String>,
    /// "use" (whatever its grade), "exclude" (keep it out), or "clear".
    pub choice: String,
}

/// PUT /api/db/{db}/calibrations/frames/quality — use frames whatever their
/// grade, keep them out of masters and exports, or clear that choice. A
/// kept-out frame stays in the library, so a rescan does not bring it back.
pub async fn set_calibration_quality_override(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<SetCalibrationQualityOverrideRequest>,
) -> Result<Json<ApiResponse<SetCalibrationValidityResponse>>, AppError> {
    let choice = match request.choice.as_str() {
        "clear" => None,
        "use" => Some(crate::calibration::QualityOverride::Use),
        "exclude" => Some(crate::calibration::QualityOverride::Exclude),
        other => {
            return Err(AppError::BadRequest(format!(
                "choice must be use, exclude, or clear, not {other:?}"
            )));
        }
    };
    if request.frame_uuids.is_empty() {
        return Err(AppError::BadRequest("no frames named".into()));
    }
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let updated = crate::calibration::set_overrides(&conn, &request.frame_uuids, choice)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    if updated == 0 {
        return Err(AppError::NotFound);
    }
    // What each project's lights get has changed.
    crate::server::calibration_health::request(&state, &ctx.database_path);
    // A flat kept out or let back in changes its session's response.
    crate::server::flat_measure::queue(
        &state,
        &ctx.database_path,
        crate::calibration::flat_groups::job::Scope::flats(
            crate::server::flat_measure::rigs_of_frames(&conn, &request.frame_uuids),
        ),
    );
    Ok(Json(ApiResponse::success(SetCalibrationValidityResponse {
        updated,
    })))
}

pub async fn clear_calibration_masters(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<crate::calibration::CalibrationMutationOutcome>>, AppError> {
    let _permit = state
        .stack_previews
        .acquire_maintenance_permit()
        .await
        .map_err(AppError::Conflict)?;
    let master_root = crate::server::storage::calibration_masters(&ctx.calibration_root);
    if master_root.exists() {
        tokio::fs::remove_dir_all(&master_root)
            .await
            .map_err(|error| {
                AppError::InternalError(format!(
                    "removing cached calibration masters at {}: {error}",
                    master_root.display()
                ))
            })?;
    }
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let outcome = crate::calibration::clear_generated_masters(&conn).map_err(AppError::db)?;
    Ok(Json(ApiResponse::success(outcome)))
}

/// Report which Seiza resources are configured and can be opened. Normal
/// capability checks are bounded header/index opens, not exhaustive scans.
pub async fn get_astrometry_capabilities(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<crate::astrometry::AstrometryCapabilities>>, AppError> {
    let astrometry = Arc::clone(&state.astrometry);
    let capabilities = tokio::task::spawn_blocking(move || astrometry.capabilities())
        .await
        .map_err(|error| {
            AppError::InternalError(format!("Astrometry capability task failed: {error}"))
        })?;
    Ok(Json(ApiResponse::success(capabilities)))
}

/// Exhaustively validate every configured Seiza catalog. This deliberately
/// runs on the blocking pool and participates in the interactive-work gauge.
pub async fn validate_astrometry_catalogs(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<crate::astrometry::AstrometryValidationReport>>, AppError> {
    let guard = state.begin_named_interactive_job("a star catalog check");
    let astrometry = Arc::clone(&state.astrometry);
    let report = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        astrometry.try_validate_all()
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Astrometry validation task failed: {error}"))
    })?
    .map_err(AppError::BadRequest)?;
    Ok(Json(ApiResponse::success(report)))
}

/// Report the current background Seiza catalog installation.
pub async fn get_astrometry_catalog_install(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<crate::server::catalog_install::CatalogInstallStatus>>, AppError> {
    Ok(Json(ApiResponse::success(state.catalog_install.status())))
}

/// Download and install one hosted Seiza catalog package. The destination is
/// the server's own configured catalog directory, so it takes no path from
/// the client and an editor may start it. The work continues after the
/// request returns.
pub async fn start_astrometry_catalog_install(
    State(state): State<Arc<AppState>>,
    Json(request): Json<crate::server::catalog_install::CatalogInstallRequest>,
) -> Result<Json<ApiResponse<crate::server::catalog_install::CatalogInstallStatus>>, AppError> {
    let output_dir = state.astrometry.catalog_install_dir();
    let guard = state.begin_named_interactive_job("a star catalog installation");
    let status = state
        .catalog_install
        .start(request.preset, output_dir, guard, &state.shutdown);
    Ok(Json(ApiResponse::success(status)))
}

/// Stop the star catalog installation under way. Files already installed
/// stay. Replies whether one was running.
pub async fn stop_astrometry_catalog_install(
    State(state): State<Arc<AppState>>,
) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(state.catalog_install.stop()))
}

/// Header-only catalog association and embedded-WCS overlay geometry for one
/// image. This stays separate from image metadata so provenance, partial
/// capability, and later plate-solve results retain a typed contract.
pub async fn get_image_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<crate::astrometry::AstrometryAnalysis>>, AppError> {
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx.0, image_id).await?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_named_interactive_job("a plate solve");
    let analysis = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        astrometry
            .analyze_image(image_id, &fits_path, expected_target)
            .map(|analysis| astrometry.with_cached_solution(&cache_dir, analysis))
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Image astrometry task failed: {error}")))?
    .map_err(AppError::BadRequest)?;
    Ok(Json(ApiResponse::success(analysis)))
}

/// Decode pixels and run Seiza's hinted solver with a blind fallback. The
/// successful WCS is persisted in the per-database cache and immediately
/// returned in the same contract consumed by the shared overlay component.
pub async fn solve_image_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<crate::astrometry::AstrometryAnalysis>>, AppError> {
    let _solve_guard = ctx.astrometry_solve_mutex.lock().await;
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx.0, image_id).await?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_named_interactive_job("a plate solve");
    let analysis = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let fresh = astrometry.analyze_image(image_id, &fits_path, expected_target)?;
        let cached = astrometry.with_cached_solution(&cache_dir, fresh);
        let (analysis, newly_solved) = if cached.solution.is_some() {
            (cached, false)
        } else {
            (
                astrometry.solve_image(image_id, &fits_path, expected_target)?,
                true,
            )
        };
        if newly_solved
            && analysis
                .solve_attempt
                .as_ref()
                .is_some_and(|attempt| attempt.cacheable)
        {
            crate::astrometry::persist_pixel_analysis(&cache_dir, &analysis)?;
        }
        Ok::<_, String>(analysis)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Plate solve task failed: {error}")))?
    .map_err(AppError::BadRequest)?;
    Ok(Json(ApiResponse::success(analysis)))
}

/// Return a source- and WCS-validated cached satellite prediction without
/// refreshing orbital elements or performing propagation.
pub async fn get_image_satellites(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<crate::satellites::SatelliteAnalysisStatus>>, AppError> {
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx.0, image_id).await?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_named_interactive_job("a satellite prediction");
    let analysis = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        astrometry
            .analyze_image(image_id, &fits_path, expected_target)
            .map(|analysis| astrometry.with_cached_solution(&cache_dir, analysis))
            .map(|astrometry| {
                crate::satellites::persisted_analysis(&cache_dir, image_id, &astrometry)
            })
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Satellite cache validation task failed: {error}"))
    })?
    .map_err(AppError::BadRequest)?;
    Ok(Json(ApiResponse::success(
        crate::satellites::SatelliteAnalysisStatus {
            analysis,
            orbital_elements_cached: state.satellites.has_cached_elements(),
        },
    )))
}

/// Ensure a pixel WCS, explicitly refresh/load orbital elements, predict all
/// clipped tracks during this single exposure, and persist the result for
/// later sequence grading. Orbital prediction and bounded pixel alignment are
/// returned as separate evidence.
pub async fn predict_image_satellites(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<crate::satellites::SatelliteAnalysis>>, AppError> {
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx.0, image_id).await?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_named_interactive_job("a satellite prediction");
    let validation_path = fits_path.clone();
    tokio::task::spawn_blocking(move || crate::satellites::validate_exposure(&validation_path))
        .await
        .map_err(|error| {
            AppError::InternalError(format!("Satellite exposure validation failed: {error}"))
        })?
        .map_err(AppError::BadRequest)?;
    let astrometry_analysis = {
        let _solve_guard = ctx.astrometry_solve_mutex.lock().await;
        let solve_path = fits_path.clone();
        let solve_cache = cache_dir.clone();
        tokio::task::spawn_blocking(move || {
            let fresh = astrometry.analyze_image(image_id, &solve_path, expected_target)?;
            let cached = astrometry.with_cached_solution(&solve_cache, fresh);
            let (analysis, newly_solved) = if cached.solution.is_some() {
                (cached, false)
            } else {
                (
                    astrometry.solve_image(image_id, &solve_path, expected_target)?,
                    true,
                )
            };
            if newly_solved
                && analysis
                    .solve_attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.cacheable)
            {
                crate::astrometry::persist_pixel_analysis(&solve_cache, &analysis)?;
            }
            Ok::<_, String>(analysis)
        })
        .await
        .map_err(|error| {
            AppError::InternalError(format!("Satellite plate solve task failed: {error}"))
        })?
        .map_err(AppError::BadRequest)?
    };

    state
        .satellites
        .load_for_exposure(&fits_path)
        .await
        .map_err(AppError::BadRequest)?;
    let satellite_context = Arc::clone(&state.satellites);
    let prediction_path = fits_path;
    let prediction_cache = cache_dir;
    let prediction = tokio::task::spawn_blocking(move || {
        let _guard = guard;
        let snapshot = satellite_context
            .cached_for_exposure(&prediction_path)?
            .ok_or_else(|| "no cached satellite elements are available".to_string())?;
        let analysis = crate::satellites::predict_tracks(
            image_id,
            &prediction_path,
            &astrometry_analysis,
            &snapshot,
        )?;
        crate::satellites::persist_analysis(&prediction_cache, &analysis)?;
        Ok::<_, String>(analysis)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Satellite prediction task failed: {error}")))?
    .map_err(AppError::BadRequest)?;
    Ok(Json(ApiResponse::success(prediction)))
}

async fn resolve_astrometry_input(
    ctx: &Arc<DatabaseContext>,
    image_id: i32,
) -> Result<(PathBuf, Option<(f64, f64)>), AppError> {
    let (image, file_only, target_name) = resolve_image_meta(ctx, image_id)?;
    let expected_target = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        crate::acquisition_context::load(&conn, &image)
            .map_err(AppError::db)?
            .expected_for_grading()
    };
    let fits_path = find_fits_file_async(ctx, &image, &target_name, &file_only).await?;
    Ok((fits_path, expected_target))
}

/// List all configured databases. Used by the frontend to populate the DB
/// switcher and resolve the default `?db=` value.
pub async fn list_databases(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<DatabaseSummary>>>, AppError> {
    let mut summaries: Vec<DatabaseSummary> = state
        .all_databases()
        .iter()
        .map(|ctx| summary_of(ctx))
        .collect();
    summaries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(ApiResponse::success(summaries)))
}

/// Load the registry, let `change` edit it, and save it. The registry write
/// lock is held throughout, so two settings saves never lose one another's
/// edit. An error from `change` saves nothing.
pub(crate) async fn update_registry<T>(
    state: &AppState,
    change: impl FnOnce(&mut crate::db_registry::DbRegistry) -> Result<T, AppError>,
) -> Result<T, AppError> {
    let path = require_registry_path(state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = crate::db_registry::DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let value = change(&mut registry)?;
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(value)
}

pub(crate) fn require_registry_path(state: &AppState) -> Result<std::path::PathBuf, AppError> {
    state.registry_path.read().unwrap().clone().ok_or_else(|| {
        AppError::BadRequest(
            "database registry persistence is not configured for this server".into(),
        )
    })
}

/// Gate for endpoints that let a caller name a server filesystem path:
/// registry CRUD, database creation, local export, and folder import. Those
/// requests choose which files the server reads and writes, so anyone
/// reachable over the network could otherwise register an arbitrary path and
/// read it back through the normal API.
///
/// Bulk target merges and exposure moves also require this opt-in. Ordinary
/// grading, scheduler field edits, exposure plans, and sync do not: those
/// touch only data the operator already pointed the server at.
pub(super) fn require_database_management_allowed(state: &AppState) -> Result<(), AppError> {
    if state.database_management_allowed() {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "database management is disabled on this server. Restart the server \
            with --allow-database-management to register or change databases, \
            or to name server folders for imports, exports, storage or PixInsight."
                .into(),
        ))
    }
}

/// A project's name for display. The old behavior prefixed EVERY project
/// with its profile id whenever two profiles existed — and scheduler
/// databases carry no friendly profile names, so users saw their projects
/// "renamed" to raw GUIDs. A short tag now appears only when the same
/// project name exists under more than one profile.
fn display_project_name(
    name: &str,
    profile_id: &str,
    ambiguous: &std::collections::HashSet<String>,
) -> String {
    if ambiguous.contains(name) {
        let tag: String = profile_id.chars().take(8).collect();
        format!("{name} \u{00b7} {tag}")
    } else {
        name.to_string()
    }
}

fn summary_of(ctx: &crate::server::database_context::DatabaseContext) -> DatabaseSummary {
    DatabaseSummary {
        ts_sync: ctx.ts_sync.clone(),
        id: ctx.id.clone(),
        name: ctx.name.clone(),
        database_path: ctx.database_path.clone(),
        image_directories: ctx.image_dirs.clone(),
        remote_image_upload: remote_image_upload_summary(ctx),
        export_directory: ctx
            .export_dir
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        process_directory: ctx
            .process_dir
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        autoimport: ctx.autoimport.clone(),
        scan_rules: (ctx.scan_rules != Default::default()).then(|| ctx.scan_rules.clone()),
    }
}

fn remote_image_upload_summary(
    ctx: &crate::server::database_context::DatabaseContext,
) -> RemoteImageUploadSummary {
    let config = ctx.remote_image_upload.as_ref();
    let directory_layout = config.and_then(|config| config.directory_layout.as_ref());
    RemoteImageUploadSummary {
        enabled: ctx.remote_image_upload_dir.is_some(),
        image_directory: config
            .map(|config| config.image_dir.clone())
            .filter(|directory| !directory.is_empty()),
        placement: config.map(|config| config.placement).unwrap_or_default(),
        directory_template: config
            .map(|config| config.fallback_directory_template().to_string())
            .unwrap_or_else(|| {
                crate::db_registry::DEFAULT_REMOTE_UPLOAD_DIRECTORY_TEMPLATE.to_string()
            }),
        catalog_directory_template: directory_layout
            .filter(|layout| {
                layout.source == crate::db_registry::RemoteImageUploadTemplateSource::Catalog
            })
            .map(|layout| layout.template.clone()),
        directory_template_source: directory_layout
            .map(|layout| layout.source)
            .unwrap_or_default(),
        directory_template_samples: directory_layout.map(|layout| layout.samples).unwrap_or(0),
        token_configured: config.is_some_and(|config| config.token_is_configured()),
        sync_enabled: config.is_some_and(|config| config.sync_enabled),
        clients: config
            .map(|config| {
                config
                    .clients
                    .iter()
                    .map(|client| RemoteClientSummary {
                        client_uuid: client.client_uuid.clone(),
                        name: client.name.clone(),
                        paired_at: client.paired_at,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// `POST /api/databases` — register a new database. Validates that the file
/// opens, persists the registry, and inserts the new `DatabaseContext` into
/// the in-memory map.
pub async fn add_database_route(
    State(state): State<Arc<AppState>>,
    Json(req): Json<AddDatabaseRequest>,
) -> Result<Json<ApiResponse<DatabaseSummary>>, AppError> {
    use crate::db_registry::DbRegistry;
    use crate::server::database_context::DatabaseContext;

    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut reg = DbRegistry::load_or_init(&registry_path)
        .map_err(|e| AppError::InternalError(format!("loading registry: {}", e)))?;

    let entry = reg
        .add(req.name, req.db_path, req.image_dirs, req.slug)
        .map_err(|e| AppError::BadRequest(e.to_string()))?
        .clone();

    // Open the connection now so persisting bad config can't outlive a failure.
    let ctx = Arc::new(
        DatabaseContext::from_entry(&entry, state.storage_roots.clone())
            .map_err(|e| AppError::BadRequest(format!("opening database: {}", e)))?,
    );

    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    // Its query indexes and readout modes, as at start.
    state.add_database(ctx.clone());

    // Kick a background refresh for the newly-registered DB so file/dir caches
    // populate without the user having to refresh manually.
    let _ = ctx.ensure_cache_available();
    // Grade its calibration frames and judge its projects, as at start, and
    // measure the flats it has not measured.
    crate::server::calibration_quality::request(&state, &ctx.database_path);
    crate::server::flat_measure::queue(
        &state,
        &ctx.database_path,
        crate::calibration::flat_groups::job::Scope::everything(),
    );

    Ok(Json(ApiResponse::success(summary_of(&ctx))))
}

/// `PUT /api/databases/{db_id}` — update name / slug / db_path / image_dirs.
/// Reopens the connection if `db_path` or `image_dirs` change. Slug rename is
/// allowed; the cache directory move is queued for B5.
pub async fn update_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
    Json(req): Json<UpdateDatabaseRequest>,
) -> Result<Json<ApiResponse<DatabaseSummary>>, AppError> {
    use crate::db_registry::DbRegistry;
    use crate::server::database_context::DatabaseContext;

    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut reg = DbRegistry::load_or_init(&registry_path)
        .map_err(|e| AppError::InternalError(format!("loading registry: {}", e)))?;

    let previous_entry = reg.find(&db_id).cloned().ok_or(AppError::NotFound)?;

    if previous_entry.ts_sync.is_some()
        && req
            .db_path
            .as_ref()
            .is_some_and(|path| path != &previous_entry.db_path)
    {
        return Err(AppError::BadRequest(
            "An imported catalog keeps its PSF Guard database path.".into(),
        ));
    }

    reg.update(
        &db_id,
        req.name.clone(),
        req.slug.clone(),
        req.db_path.clone(),
        req.image_dirs.clone(),
    )
    .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // Pull the entry back out (post-rename) so we can rebuild the context.
    let new_id = req.slug.clone().unwrap_or_else(|| db_id.clone());
    if let Some(settings) = req.ts_sync.as_ref() {
        if previous_entry
            .ts_sync
            .as_ref()
            .is_none_or(|previous| previous.source_path != settings.source_path)
        {
            return Err(AppError::BadRequest("Import a Target Scheduler file to create its sync connection. An existing connection cannot be changed to another source.".into()));
        }
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::NotFound)?;
        settings
            .validate(std::path::Path::new(&entry.db_path))
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
        entry.ts_sync = Some(settings.clone());
    }
    let mut layout_chosen = false;
    if let Some(update) = req.remote_image_upload.as_ref() {
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        layout_chosen = apply_remote_image_upload_update(entry, update)?;
    }
    let updated_entry = reg.find(&new_id).ok_or(AppError::InternalError(
        "registry update lost the entry".into(),
    ))?;
    let rescan_requested = req
        .remote_image_upload
        .as_ref()
        .is_some_and(|update| update.rescan_directory_layout);
    let remote_layout_scan = updated_entry
        .remote_image_upload
        .as_ref()
        .filter(|config| {
            config.enabled
                && config.placement == crate::db_registry::RemoteImageUploadPlacement::TargetTree
                // A scan whose result a layout chosen in this save would
                // discard is not run: it walks the receive root under the
                // registry write lock. An explicit rescan is run and wins.
                && (!layout_chosen || rescan_requested)
                && remote_image_layout_needs_scan(
                    &previous_entry,
                    updated_entry,
                    req.remote_image_upload.as_ref(),
                )
        })
        .map(|config| {
            config
                .validated_image_dir(&updated_entry.image_dirs)
                .map(|receive_root| (updated_entry.db_path.clone(), receive_root))
                .map_err(|error| AppError::BadRequest(error.to_string()))
        })
        .transpose()?;
    if let Some((database_path, receive_root)) = remote_layout_scan {
        let detected = tokio::task::spawn_blocking(move || {
            crate::server::remote_upload_layout::detect_catalog_directory_layout(
                &database_path,
                &receive_root,
            )
        })
        .await
        .map_err(|error| {
            AppError::InternalError(format!("remote upload layout scan failed: {error}"))
        })?;
        let detected = match detected {
            Ok(detected) => detected,
            Err(error) => {
                tracing::warn!(
                    database = %new_id,
                    "Could not detect remote upload catalog layout; using preset: {error:#}"
                );
                None
            }
        };
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        let config = entry
            .remote_image_upload
            .as_mut()
            .expect("layout scanning requires remote upload configuration");
        adopt_detected_layout(config, detected, layout_chosen && !rescan_requested);
    }
    if let Some(export_dir) = req.export_dir.as_ref() {
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        let trimmed = export_dir.trim();
        entry.export_dir = if trimmed.is_empty() {
            None
        } else {
            if std::path::Path::new(trimmed).is_relative() {
                return Err(AppError::BadRequest(
                    "Export directory must be an absolute path".into(),
                ));
            }
            Some(trimmed.to_string())
        };
    }
    if let Some(process_dir) = req.process_dir.as_ref() {
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        let trimmed = process_dir.trim();
        entry.process_dir = if trimmed.is_empty() {
            None
        } else {
            if std::path::Path::new(trimmed).is_relative() {
                return Err(AppError::BadRequest(
                    "Process directory must be an absolute path".into(),
                ));
            }
            Some(trimmed.to_string())
        };
    }
    if let Some(autoimport) = req.autoimport.as_ref() {
        autoimport
            .validate()
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        // Off with default choices reads as unset, so a registry that never
        // heard of the feature stays as it was.
        entry.autoimport = if *autoimport == crate::db_registry::AutoImportSettings::default() {
            None
        } else {
            Some(autoimport.clone())
        };
    }
    if let Some(scan_rules) = req.scan_rules.as_ref() {
        let scan_rules = scan_rules.normalized();
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        let roots: Vec<PathBuf> = entry.image_dirs.iter().map(PathBuf::from).collect();
        scan_rules
            .validate(&roots)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        // Rules that change nothing are stored as none.
        entry.scan_rules = (scan_rules != Default::default()).then_some(scan_rules);
    }
    let entry = reg
        .find(&new_id)
        .ok_or(AppError::InternalError(
            "registry update lost the entry".into(),
        ))?
        .clone();

    // If the slug changed, move the database's folders so previews, stacks
    // and masters carry over to the new identity. Failure is non-fatal:
    // worst case they are generated again under the new slug.
    let renamed = (new_id != db_id).then(|| {
        crate::server::storage::relocate::rename_database(&state.storage_roots, &db_id, &new_id)
    });
    for failure in renamed.iter().flat_map(|renamed| &renamed.failures) {
        tracing::warn!("Renaming {db_id} to {new_id}: {failure}");
    }

    let new_ctx = Arc::new(
        DatabaseContext::from_entry(&entry, state.storage_roots.clone())
            .map_err(|e| AppError::BadRequest(format!("opening database: {}", e)))?,
    );
    if renamed.is_some_and(|renamed| renamed.calibration_moved) {
        crate::server::storage::relocate::follow_master_rows(
            &new_ctx,
            &state.storage_roots.calibration.join(&db_id),
            &new_ctx.calibration_root,
        );
    }

    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    // If slug changed, remove the old entry.
    if new_id != db_id {
        state.databases.write().unwrap().remove(&db_id);
    }
    state.add_database(new_ctx.clone());

    let _ = new_ctx.ensure_cache_available();
    // Lights may be found elsewhere, or found at last.
    if previous_entry.image_dirs != entry.image_dirs {
        crate::server::calibration_health::request(&state, &new_ctx.database_path);
    }

    Ok(Json(ApiResponse::success(summary_of(&new_ctx))))
}

/// Apply a management client's receiver settings. Returns whether the
/// request chose a folder layout, which a catalog scan in the same save must
/// not override.
fn apply_remote_image_upload_update(
    entry: &mut crate::db_registry::DbEntry,
    update: &RemoteImageUploadUpdate,
) -> Result<bool, AppError> {
    let mut config = entry.remote_image_upload.clone().unwrap_or_default();
    config.enabled = update.enabled;
    if let Some(directory) = update.image_directory.as_ref() {
        config.image_dir = directory.trim().to_string();
    }
    if let Some(token) = update.token.as_ref() {
        config
            .set_token(token)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
    }
    if let Some(sync_enabled) = update.sync_enabled {
        config.sync_enabled = sync_enabled;
    }
    let target_tree_was_selected = update.placement.is_some_and(|placement| {
        placement == crate::db_registry::RemoteImageUploadPlacement::TargetTree
            && config.placement != crate::db_registry::RemoteImageUploadPlacement::TargetTree
    });
    if let Some(placement) = update.placement {
        config.placement = placement;
    }
    let mut layout_chosen = false;
    if let Some(template) = update
        .directory_template
        .as_deref()
        // In catalog mode a template, if a client sends one anyway, is not
        // used for anything, so it is neither validated nor stored.
        .filter(|_| {
            update.directory_template_source
                != Some(crate::db_registry::RemoteImageUploadTemplateSource::Catalog)
        })
    {
        use crate::db_registry::{
            RemoteImageUploadDirectoryLayout, RemoteImageUploadTemplateSource,
        };
        crate::server::remote_upload_layout::validate_directory_template(template)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
        let template = template.trim().replace('\\', "/");
        // Is this template a choice, and does it hold against a catalog scan
        // in the same save?
        //
        // A client that says `preset` chose it — when it changes what is in
        // force. The form echoes the summary's template on every save, and
        // an echo must not turn a legacy entry with no layout into an
        // explicit one (that moves where calibration frames land). A real
        // choice holds against a scan even on the save that first switches
        // to the catalog tree: the form only says `preset` for a layout the
        // person touched.
        //
        // A client that says nothing is older than the field: as before
        // 0.9.3 a template counts when it differs from the stored fallback
        // (or the tree was just selected), and on that first-tree save the
        // first detection still decides, because such a client cannot tell
        // an untouched default from a pick.
        let (chosen, holds) = match update.directory_template_source {
            Some(RemoteImageUploadTemplateSource::Preset) => {
                let changes = template != config.directory_template()
                    || config.directory_layout.is_none() && target_tree_was_selected
                    || config.directory_layout.as_ref().is_some_and(|layout| {
                        layout.source == RemoteImageUploadTemplateSource::Catalog
                    });
                (changes, true)
            }
            Some(RemoteImageUploadTemplateSource::Catalog) => (false, false),
            None => (
                target_tree_was_selected || template != config.fallback_directory_template(),
                !target_tree_was_selected,
            ),
        };
        if chosen {
            // The chosen layout is the layout: it replaces a catalog match
            // rather than hiding behind it as the fallback (#399).
            config.directory_layout = Some(RemoteImageUploadDirectoryLayout {
                template: template.clone(),
                fallback_template: template,
                source: RemoteImageUploadTemplateSource::Preset,
                samples: 0,
            });
            layout_chosen = holds;
        }
    }
    if config.sync_enabled && !config.token_is_configured() {
        return Err(AppError::BadRequest(
            "generate a remote API key before enabling remote scheduler sync".into(),
        ));
    }
    if config.enabled {
        config
            .validated_image_dir(&entry.image_dirs)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
    }
    entry.remote_image_upload = Some(config);
    Ok(layout_chosen)
}

/// Fold a catalog scan's result into the receiver settings. A layout chosen
/// in the same request wins over whatever the scan found: the person asked
/// for that layout, and a scan that promoted the catalog match back over it
/// is exactly what #399 reported. An empty or failed scan returns a catalog
/// layout to its fallback and leaves a chosen one alone.
fn adopt_detected_layout(
    config: &mut crate::db_registry::RemoteImageUploadConfig,
    detected: Option<crate::server::remote_upload_layout::DetectedDirectoryLayout>,
    layout_chosen: bool,
) {
    // The caller passes `layout_chosen = false` for an explicit rescan: the
    // person asked to match the catalog again, and that is the later wish.
    if layout_chosen {
        return;
    }
    match detected {
        Some(detected) => {
            config.directory_layout = Some(crate::db_registry::RemoteImageUploadDirectoryLayout {
                template: detected.template,
                fallback_template: config.fallback_directory_template().to_string(),
                source: crate::db_registry::RemoteImageUploadTemplateSource::Catalog,
                samples: detected.samples,
            });
        }
        None => reset_catalog_directory_layout(config),
    }
}

fn remote_image_layout_needs_scan(
    previous: &crate::db_registry::DbEntry,
    updated: &crate::db_registry::DbEntry,
    request: Option<&RemoteImageUploadUpdate>,
) -> bool {
    if request.is_some_and(|request| request.rescan_directory_layout) {
        return true;
    }
    // A changed layout choice is deliberately not a trigger: choosing a
    // layout is a decision, not a reason to go looking for another. (It used
    // to be, and the scan promoted the catalog match back over the layout
    // just chosen — #399.)
    let previous_config = previous.remote_image_upload.as_ref();
    let updated_config = updated.remote_image_upload.as_ref();
    previous.db_path != updated.db_path
        || previous.image_dirs != updated.image_dirs
        || previous_config.map(|config| config.enabled)
            != updated_config.map(|config| config.enabled)
        || previous_config.map(|config| config.image_dir.trim())
            != updated_config.map(|config| config.image_dir.trim())
        || previous_config.map(|config| config.placement)
            != updated_config.map(|config| config.placement)
}

fn reset_catalog_directory_layout(config: &mut crate::db_registry::RemoteImageUploadConfig) {
    if config.directory_layout.as_ref().is_some_and(|layout| {
        layout.source == crate::db_registry::RemoteImageUploadTemplateSource::Catalog
    }) {
        let fallback = config.fallback_directory_template().to_string();
        config.directory_layout = Some(crate::db_registry::RemoteImageUploadDirectoryLayout {
            template: fallback.clone(),
            fallback_template: fallback,
            source: crate::db_registry::RemoteImageUploadTemplateSource::Preset,
            samples: 0,
        });
    }
}

#[cfg(test)]
mod remote_image_layout_settings_tests;

/// `DELETE /api/databases/{db_id}` — drop the registered database. Returns
/// 200 with `{removed: true}` even on first-call (idempotent on the surface).
pub async fn remove_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    use crate::db_registry::DbRegistry;

    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut reg = DbRegistry::load_or_init(&registry_path)
        .map_err(|e| AppError::InternalError(format!("loading registry: {}", e)))?;
    let removed_from_registry = reg
        .remove(&db_id)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;
    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    let removed_from_state = state.databases.write().unwrap().remove(&db_id).is_some();

    Ok(Json(ApiResponse::success(serde_json::json!({
        "removed": removed_from_registry || removed_from_state,
    }))))
}

fn nights_view(raw: Option<&str>) -> Option<crate::calibration::LightNightSplit> {
    raw.and_then(crate::calibration::LightNightSplit::parse)
}

/// `GET /api/db/{db_id}/projects/{project_id}/calibration-report` — how the
/// calibration library covers one project's lights: what matches, how old
/// it is, and whether each night has its own flats.
pub async fn get_project_calibration_report(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Query(query): Query<CalibrationReportQuery>,
) -> Result<Json<ApiResponse<crate::calibration::ProjectCalibrationReport>>, AppError> {
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| AppError::InternalError(format!("indexing image folders: {error}")))?;
    let database_path = ctx.database_path.clone();
    let report = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| anyhow::anyhow!("opening {database_path}: {error}"))?;
        crate::calibration::project_calibration_report(
            &conn,
            project_id,
            &directory_tree,
            query.include_rejected,
            nights_view(query.nights.as_deref()),
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("report task: {error}")))?
    .map_err(|error| AppError::InternalError(format!("calibration report: {error:#}")))?;
    Ok(Json(ApiResponse::success(report)))
}

/// Read the project's lights against the library on a blocking thread, from
/// a read-only connection, as the report does.
async fn project_calibration_gaps(
    ctx: &DbContext,
    project_id: i32,
    nights: Option<crate::calibration::LightNightSplit>,
) -> Result<crate::calibration::ProjectCalibrationGaps, AppError> {
    let directory_tree = ctx
        .get_directory_tree()
        .map_err(|error| AppError::InternalError(format!("indexing image folders: {error}")))?;
    let database_path = ctx.database_path.clone();
    tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| anyhow::anyhow!("opening {database_path}: {error}"))?;
        crate::calibration::project_calibration_gaps(&conn, project_id, &directory_tree, nights)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("calibration check task: {error}")))?
    .map_err(|error| AppError::InternalError(format!("calibration check: {error:#}")))
}

/// `GET /api/db/{db_id}/projects/{project_id}/calibration-report/rejects` —
/// every light, rejected ones aside, that a stack leaves out because the
/// library cannot calibrate it properly, with a digest naming the list.
pub async fn get_project_calibration_gaps(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Query(query): Query<NightsQuery>,
) -> Result<Json<ApiResponse<crate::calibration::ProjectCalibrationGaps>>, AppError> {
    Ok(Json(ApiResponse::success(
        project_calibration_gaps(&ctx, project_id, nights_view(query.nights.as_deref())).await?,
    )))
}

#[derive(Debug, Deserialize)]
pub struct RejectUncalibratedRequest {
    /// The digest of the list the person saw.
    pub digest: String,
}

/// `POST /api/db/{db_id}/projects/{project_id}/calibration-report/rejects` —
/// reject the lights that cannot be calibrated, so Target Scheduler shoots
/// them again. Refuses with 409 when the list is no longer the one shown.
pub async fn reject_uncalibrated_lights(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(request): Json<RejectUncalibratedRequest>,
) -> Result<Json<ApiResponse<BatchGradeResponse>>, AppError> {
    // The digest is the same whichever split names the nights.
    let gaps = project_calibration_gaps(&ctx, project_id, None).await?;
    if gaps.digest != request.digest {
        return Err(AppError::Conflict(
            "The lights that cannot be calibrated changed since the check; check again".into(),
        ));
    }
    let updates = gaps
        .lights
        .into_iter()
        .map(|light| {
            (
                light.image_id,
                GradingStatus::Rejected,
                Some(format!("Cannot be calibrated: {}", light.reason)),
            )
        })
        .collect::<Vec<_>>();
    Ok(Json(ApiResponse::success(apply_grades(
        &state, &ctx, &updates,
    )?)))
}

/// `GET /api/db/{db_id}/images/{image_id}/calibration` — why the light got
/// the masters it did: every frame the library holds for its camera, used,
/// matching but unused, or refused with the readings that disagree.
pub async fn get_image_calibration(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(query): Query<NightsQuery>,
) -> Result<Json<ApiResponse<crate::calibration::CalibrationExplanation>>, AppError> {
    let (image, filename, target_name) = resolve_image_meta(&ctx, image_id)?;
    let path = find_fits_file(&ctx, &image, &target_name, &filename)?;
    // The light's night as the grid names it, so the panel's same-night
    // flats and nights apart count from the night the light was opened in.
    let light_night = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let nights = crate::server::sky_coverage::cached_light_nights(
            &ctx,
            &conn,
            nights_view(query.nights.as_deref()),
        )?;
        image
            .acquired_date
            .map(|at| nights.night_of(image.profile_id.as_deref(), at))
    };
    let database_path = ctx.database_path.clone();
    let explanation = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| anyhow::anyhow!("opening {database_path}: {error}"))?;
        let light = crate::commands::import::headers::read_frame_meta(&path);
        crate::calibration::explain_calibration(&conn, &light)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("calibration explanation task: {error}")))?
    .map_err(|error| AppError::InternalError(format!("explaining calibration: {error:#}")))?;
    let mut explanation = explanation;
    if light_night.is_some() {
        explanation.light.night = light_night;
    }
    Ok(Json(ApiResponse::success(explanation)))
}

/// `PUT /api/db/{db_id}/projects/{project_id}` — update scheduler fields.
pub async fn update_project_route(
    State(_state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    crate::server::scheduler::update_project(ctx, project_id, req)?;
    Ok(Json(ApiResponse::success(
        serde_json::json!({ "updated": true }),
    )))
}

/// What `GET /api/db/{db_id}/guids` reports: each Target Scheduler table's
/// rows without a GUID, and whether this server could fill them.
#[derive(Debug, Serialize)]
pub struct GuidReport {
    pub tables: Vec<crate::ts_guids::GuidGap>,
    pub missing: u64,
    pub writable: bool,
}

/// What `POST /api/db/{db_id}/guids/fill` did: rows given a GUID, per
/// table, and the copy taken first (none when nothing was missing).
#[derive(Debug, Serialize)]
pub struct GuidFillReport {
    pub tables: Vec<crate::ts_guids::GuidGap>,
    pub filled: u64,
    pub backup_path: Option<String>,
}

fn guid_error(error: anyhow::Error) -> AppError {
    AppError::InternalError(format!("{error:#}"))
}

/// `GET /api/db/{db_id}/guids` — rows Target Scheduler's migration left
/// without a GUID (see `ts_guids`). Read only.
pub async fn missing_guids_route(
    ctx: DbContext,
) -> Result<Json<ApiResponse<GuidReport>>, AppError> {
    let path = std::path::PathBuf::from(&ctx.database_path);
    let report = tokio::task::spawn_blocking(move || -> anyhow::Result<GuidReport> {
        let conn = crate::ts_guids::open_existing(&path, false)?;
        let tables = crate::ts_guids::missing_guids(&conn)?;
        Ok(GuidReport {
            missing: crate::ts_guids::total_missing(&tables),
            tables,
            writable: crate::ts_guids::is_writable(&path),
        })
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))?
    .map_err(guid_error)?;
    Ok(Json(ApiResponse::success(report)))
}

/// `POST /api/db/{db_id}/guids/fill` — copy the database beside itself, then
/// give every row without a GUID a new one. An editor's repair of catalog
/// rows; the copy goes beside the configured file. Runs off the request
/// path on its own connections, so the shared one stays free for the
/// minutes a large copy over the network can take.
pub async fn fill_guids_route(
    ctx: DbContext,
) -> Result<Json<ApiResponse<GuidFillReport>>, AppError> {
    let path = std::path::PathBuf::from(&ctx.database_path);
    let outcome = tokio::task::spawn_blocking(move || crate::ts_guids::fill_database(&path))
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?
        .map_err(guid_error)?;
    let filled = crate::ts_guids::total_missing(&outcome.filled);
    if let Some(backup) = &outcome.backup {
        tracing::info!(db = %ctx.id, filled, backup = %backup.display(), "filled missing Target Scheduler GUIDs");
    }
    Ok(Json(ApiResponse::success(GuidFillReport {
        tables: outcome.filled,
        filled,
        backup_path: outcome.backup.map(|backup| backup.display().to_string()),
    })))
}

/// `PUT /api/db/{db_id}/targets/{target_id}` — rename a target and/or move it
/// to another project (same profile; images follow the target).
pub async fn update_target_route(
    State(_state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, target_id)): Path<(String, i32)>,
    Json(req): Json<UpdateTargetRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    if req.name.is_none()
        && req.project_id.is_none()
        && req.active.is_none()
        && req.ra_hours.is_none()
        && req.dec_degrees.is_none()
        && req.epoch_code.is_none()
        && req.rotation.is_none()
        && req.roi.is_none()
    {
        return Err(AppError::BadRequest(
            "nothing to update: pass name and/or project_id".into(),
        ));
    }

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    if let Some(name) = &req.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::BadRequest("name must not be empty".into()));
        }
        if !db.rename_target(target_id, name).map_err(AppError::db)? {
            return Err(AppError::BadRequest(format!(
                "target {} not found",
                target_id
            )));
        }
    }

    let mut images_moved = 0usize;
    if let Some(project_id) = req.project_id {
        images_moved = db
            .move_target(target_id, project_id)
            .map_err(|e| AppError::BadRequest(e.to_string()))?;
    }

    crate::server::scheduler::update_target_fields(&db, target_id, &req)?;

    Ok(Json(ApiResponse::success(serde_json::json!({
        "updated": true,
        "images_moved": images_moved,
    }))))
}

/// `POST /api/db/{db_id}/projects/{project_id}/merge` — merge this project's
/// targets and images into another project, then delete it.
pub async fn merge_project_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(req): Json<MergeProjectRequest>,
) -> Result<Json<ApiResponse<MergeProjectResponse>>, AppError> {
    let (targets_moved, images_moved) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        Database::new(&conn)
            .merge_projects(project_id, req.into_project_id)
            .map_err(|e| AppError::BadRequest(e.to_string()))?
    };
    // The merged project's stacks follow it, or the stack sweep would take
    // them as a gone project's.
    let stack_ctx = ctx.0.clone();
    let into = req.into_project_id;
    let _ = tokio::task::spawn_blocking(move || {
        crate::server::stack_preview::move_project_stacks(&stack_ctx, project_id, into)
    })
    .await;
    // Targets and frames changed projects: the cards of both are out of date.
    state.auto_stacks.touch_database(
        &ctx.id,
        crate::server::stack_preview::automatic::RefreshReason::Sync,
    );
    Ok(Json(ApiResponse::success(MergeProjectResponse {
        targets_moved,
        images_moved,
    })))
}

/// `POST /api/databases/create` — bootstrap a brand-new Target Scheduler
/// database (vendored schema), register it, and start a background import of
/// the given image directories. Gated like the other management routes.
pub async fn create_database_route(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateDatabaseRequest>,
) -> Result<Json<ApiResponse<CreateDatabaseResponse>>, AppError> {
    use crate::db_registry::DbRegistry;

    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;

    if req.image_dirs.is_empty() {
        return Err(AppError::BadRequest(
            "image_dirs must contain at least one directory".into(),
        ));
    }
    for dir in &req.image_dirs {
        if !std::path::Path::new(dir).is_dir() {
            return Err(AppError::BadRequest(format!(
                "image directory does not exist: {}",
                dir
            )));
        }
    }

    let _registry_guard = state.registry_write.lock().await;
    let mut reg = DbRegistry::load_or_init(&registry_path)
        .map_err(|e| AppError::InternalError(format!("loading registry: {}", e)))?;

    // Resolve where the new .sqlite lives: explicit path, or a managed file
    // under the registry's directory named after the database.
    let db_path = match &req.db_path {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let base = registry_path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .join("databases");
            let stem = sanitize_file_stem(req.slug.as_deref().unwrap_or(&req.name));
            unique_db_file(&base, &stem)
        }
    };

    // Bootstrap on a blocking thread (fast, but it is filesystem + SQL work).
    let bootstrap_path = db_path.clone();
    tokio::task::spawn_blocking(move || crate::ts_schema::create_fresh_db(&bootstrap_path))
        .await
        .map_err(|e| AppError::InternalError(format!("bootstrap task failed: {}", e)))?
        .map_err(|e| AppError::BadRequest(format!("creating database: {}", e)))?;

    let entry = reg
        .add(
            req.name.clone(),
            db_path.to_string_lossy().into_owned(),
            req.image_dirs.clone(),
            req.slug.clone(),
        )
        .map_err(|e| AppError::BadRequest(e.to_string()))?
        .clone();

    let ctx = Arc::new(
        DatabaseContext::from_entry(&entry, state.storage_roots.clone())
            .map_err(|e| AppError::InternalError(format!("opening new database: {}", e)))?,
    );

    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    state.add_database(ctx.clone());

    let options = crate::commands::import::ImportOptions {
        time_gap_days: req
            .time_gap_days
            .unwrap_or(crate::commands::import::grouping::DEFAULT_TIME_GAP_DAYS),
        profile_id: req.profile_id.clone(),
        dry_run: false,
        ..Default::default()
    };
    spawn_import_job(
        &state,
        ctx.clone(),
        req.image_dirs.clone(),
        options,
        req.backfill.unwrap_or(false),
        req.fill_metadata.unwrap_or(true),
    );

    Ok(Json(ApiResponse::success(CreateDatabaseResponse {
        database: summary_of(&ctx),
        import: crate::server::import_job::progress_snapshot(&ctx.import_job),
    })))
}

pub async fn start_quality_backfill_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<QualityBackfillRequest>,
) -> Result<Json<ApiResponse<QualityBackfillStatusResponse>>, AppError> {
    let target_ids = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        Database::new(&conn)
            .get_all_targets_with_project_info()
            .map_err(AppError::db)?
            .into_iter()
            .map(|target| target.target.id)
            .collect::<Vec<_>>()
    };
    let started = spawn_quality_backfill(
        &state,
        ctx.0.clone(),
        target_ids,
        req.force,
        req.fill_metadata.unwrap_or(true),
    );
    Ok(Json(ApiResponse::success(QualityBackfillStatusResponse {
        started,
        progress: crate::server::quality_backfill::snapshot(&ctx.quality_backfill),
        analyze_new_frames: ctx.analyze_new_frames.is_on(),
    })))
}

#[derive(Debug, Deserialize)]
pub struct AnalyzeNewFramesRequest {
    pub enabled: bool,
}

/// `PUT /api/db/{db}/analysis/quality-backfill/new-frames` — whether frames
/// arriving by sync, upload or auto-import are analyzed for quality. A
/// database setting, so it sits behind database management like the rest.
pub async fn update_analyze_new_frames(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<AnalyzeNewFramesRequest>,
) -> Result<Json<ApiResponse<QualityBackfillStatusResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let db_id = ctx.id.clone();
    update_registry(&state, |registry| {
        let entry = registry
            .databases
            .iter_mut()
            .find(|entry| entry.id == db_id)
            .ok_or(AppError::NotFound)?;
        entry.analyze_new_frames = request.enabled;
        Ok(())
    })
    .await?;
    // The live context, which a database edit may have replaced meanwhile.
    if let Some(live) = state.get_database(&db_id) {
        live.analyze_new_frames.set(request.enabled);
    }
    ctx.analyze_new_frames.set(request.enabled);
    if request.enabled {
        // Frames already waiting are analyzed as if they had just arrived.
        crate::server::quality_arrival::note_arrival(&state, &ctx.id);
    }
    get_quality_backfill_progress(ctx).await
}

#[derive(Debug, Serialize)]
pub struct CalibratedCopiesResponse {
    /// Pair calibrated and registered copies with their lights.
    pub pair: bool,
    /// Measure quality on calibrated copies instead of raw files.
    pub scan_calibrated: bool,
    pub counts: crate::frame_derivatives::PairingCounts,
}

#[derive(Debug, Deserialize)]
pub struct CalibratedCopiesRequest {
    #[serde(default)]
    pub pair: Option<bool>,
    #[serde(default)]
    pub scan_calibrated: Option<bool>,
}

/// `GET /api/db/{db}/calibrated-copies` — whether this database pairs
/// calibrated and registered copies with their lights, and how many it has.
pub async fn get_calibrated_copies(
    ctx: DbContext,
) -> Result<Json<ApiResponse<CalibratedCopiesResponse>>, AppError> {
    let path = ctx.database_path.clone();
    let counts = tokio::task::spawn_blocking(move || {
        let conn = crate::server::database_context::open_scheduler_connection_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        crate::frame_derivatives::pairing_counts(&conn)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("counting pairings: {error}")))?
    .map_err(|error| AppError::InternalError(format!("counting pairings: {error:#}")))?;
    Ok(Json(ApiResponse::success(CalibratedCopiesResponse {
        pair: ctx.pair_calibrated_copies.is_on(),
        scan_calibrated: ctx.scan_calibrated_copies.is_on(),
        counts,
    })))
}

/// `PUT /api/db/{db}/calibrated-copies` — turn pairing, or quality
/// measured on calibrated copies, on or off. Database settings, so they sit
/// behind database management. A field left out keeps its value.
pub async fn update_calibrated_copies(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<CalibratedCopiesRequest>,
) -> Result<Json<ApiResponse<CalibratedCopiesResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let db_id = ctx.id.clone();
    update_registry(&state, |registry| {
        let entry = registry
            .databases
            .iter_mut()
            .find(|entry| entry.id == db_id)
            .ok_or(AppError::NotFound)?;
        if let Some(pair) = request.pair {
            entry.pair_calibrated_copies = pair;
        }
        if let Some(scan) = request.scan_calibrated {
            entry.scan_calibrated_copies = scan;
        }
        Ok(())
    })
    .await?;
    let live = state.get_database(&db_id);
    for context in live.iter().map(|live| &**live).chain([&*ctx.0]) {
        if let Some(pair) = request.pair {
            context.pair_calibrated_copies.set(pair);
        }
        if let Some(scan) = request.scan_calibrated {
            context.scan_calibrated_copies.set(scan);
        }
    }
    get_calibrated_copies(ctx).await
}

pub async fn get_quality_backfill_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<QualityBackfillStatusResponse>>, AppError> {
    let progress = crate::server::quality_backfill::snapshot(&ctx.quality_backfill);
    Ok(Json(ApiResponse::success(QualityBackfillStatusResponse {
        started: progress.running,
        progress,
        analyze_new_frames: ctx.analyze_new_frames.is_on(),
    })))
}

pub async fn refresh_file_cache(
    ctx: DbContext,
) -> Result<Json<ApiResponse<FileCheckResponse>>, AppError> {
    tracing::info!("🔄 Manual cache refresh requested");

    // Check current refresh status; the refresh is this person's.
    let refresh_status = ctx.request_cache_refresh();

    match refresh_status {
        crate::server::state::RefreshStatus::InProgressWait
        | crate::server::state::RefreshStatus::InProgressServeStale => {
            // A refresh is already in progress - wait for it to complete
            tracing::info!("🔄 Cache refresh already in progress, waiting for completion...");

            // Wait for the current refresh to complete by polling
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                let cache = ctx.file_check_cache.read().unwrap();
                if !cache.refresh_in_progress {
                    break;
                }
            }
        }
        crate::server::state::RefreshStatus::NeedsRefresh => {
            // A refresh was needed and should have been started by ensure_cache_available
            tracing::info!("🔄 New cache refresh started, waiting for completion...");

            // Wait for completion
            loop {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                let cache = ctx.file_check_cache.read().unwrap();
                if !cache.refresh_in_progress {
                    break;
                }
            }
        }
        crate::server::state::RefreshStatus::NotNeeded => {
            // Cache is fresh, but user requested refresh - force a new one
            tracing::info!("🔄 Cache is fresh but forced refresh requested");

            // Force a refresh by clearing cache and then starting refresh
            {
                let mut cache = ctx.file_check_cache.write().unwrap();
                cache.clear();
            }

            // Now start refresh, as this person's
            let refresh_status = ctx.request_cache_refresh();
            if matches!(
                refresh_status,
                crate::server::state::RefreshStatus::InProgressWait
                    | crate::server::state::RefreshStatus::InProgressServeStale
            ) {
                // Wait for completion
                loop {
                    tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                    let cache = ctx.file_check_cache.read().unwrap();
                    if !cache.refresh_in_progress {
                        break;
                    }
                }
            }
        }
    }

    // Get final statistics after refresh completion
    let cache = ctx.file_check_cache.read().unwrap();
    let projects_with_files = cache
        .projects_with_files
        .values()
        .filter(|&&has_files| has_files)
        .count();
    let targets_with_files = cache
        .targets_with_files
        .values()
        .filter(|&&has_files| has_files)
        .count();
    let total_checked = cache.projects_with_files.len() + cache.targets_with_files.len();
    let total_found = projects_with_files + targets_with_files;
    let total_missing = total_checked - total_found;

    let response = FileCheckResponse {
        images_checked: total_checked,
        files_found: total_found,
        files_missing: total_missing,
        check_time_ms: 0, // We don't track the wait time here
    };

    tracing::info!(
        "✅ Cache refresh completed - {} items checked, {} with files, {} missing",
        total_checked,
        total_found,
        total_missing
    );

    Ok(Json(ApiResponse::success(response)))
}

pub async fn get_cache_refresh_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<CacheRefreshProgressResponse>>, AppError> {
    let progress_info = ctx.get_cache_refresh_progress();

    let response = match progress_info {
        Some(progress) => {
            let stage_name = match progress.stage {
                crate::server::state::RefreshStage::Idle => "idle",
                crate::server::state::RefreshStage::InitializingDirectoryTree => {
                    "initializing_directory_tree"
                }
                crate::server::state::RefreshStage::LoadingProjects => "loading_projects",
                crate::server::state::RefreshStage::ProcessingProjects => "processing_projects",
                crate::server::state::RefreshStage::ProcessingTargets => "processing_targets",
                crate::server::state::RefreshStage::UpdatingCache => "updating_cache",
                crate::server::state::RefreshStage::Completed => "completed",
            };

            CacheRefreshProgressResponse {
                is_refreshing: true,
                stage: stage_name.to_string(),
                progress_percentage: progress.get_progress_percentage(),
                elapsed_seconds: progress.get_elapsed_time().map(|d| d.as_secs()),
                directories_total: progress.directories_total,
                directories_processed: progress.directories_processed,
                current_directory_name: progress.current_directory_name.clone(),
                files_scanned: progress.files_scanned,
                projects_total: progress.projects_total,
                projects_processed: progress.projects_processed,
                current_project_name: progress.current_project_name.clone(),
                targets_total: progress.targets_total,
                targets_processed: progress.targets_processed,
                files_found: progress.files_found,
                files_missing: progress.files_missing,
                automatic: !progress.requested,
            }
        }
        None => {
            // No refresh in progress
            CacheRefreshProgressResponse {
                is_refreshing: false,
                stage: "idle".to_string(),
                progress_percentage: 0.0,
                elapsed_seconds: None,
                directories_total: 0,
                directories_processed: 0,
                current_directory_name: None,
                files_scanned: 0,
                projects_total: 0,
                projects_processed: 0,
                current_project_name: None,
                targets_total: 0,
                targets_processed: 0,
                files_found: 0,
                files_missing: 0,
                automatic: false,
            }
        }
    };

    Ok(Json(ApiResponse::success(response)))
}

pub async fn refresh_directory_tree_cache(
    ctx: DbContext,
) -> Result<Json<ApiResponse<DirectoryTreeResponse>>, AppError> {
    tracing::info!("🌳 Directory tree cache refresh requested via singleton system");

    // Force directory tree refresh via singleton system (non-blocking)
    let refresh_status = ctx.force_directory_tree_refresh();

    match refresh_status {
        crate::server::state::RefreshStatus::InProgressWait
        | crate::server::state::RefreshStatus::InProgressServeStale => {
            tracing::info!("🔄 Directory tree refresh started via singleton system");
        }
        crate::server::state::RefreshStatus::NeedsRefresh => {
            tracing::info!("🔄 Directory tree refresh was needed and started");
        }
        crate::server::state::RefreshStatus::NotNeeded => {
            // This shouldn't happen since we cleared the cache, but handle it
            tracing::info!("🌳 Directory tree refresh not needed (unexpected)");
        }
    }

    // Return immediate response with basic info since this is now non-blocking
    // The actual directory tree will be built in the background with progress tracking
    let response = DirectoryTreeResponse {
        total_files: 0, // Will be updated when refresh completes
        unique_filenames: 0,
        total_directories: 0,
        age_seconds: 0,
        build_time_ms: 0, // Non-blocking, so no build time to report
        root_directory: ctx.image_dirs.join(", "),
    };

    tracing::info!(
        "✅ Directory tree cache refresh initiated (non-blocking) - check progress via /api/cache-progress"
    );

    Ok(Json(ApiResponse::success(response)))
}

pub async fn list_projects(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<ProjectResponse>>>, AppError> {
    tracing::debug!("📋 Listing projects");

    // Ensure cache is available (start refresh if needed)
    let refresh_status = ctx.ensure_cache_available();

    match refresh_status {
        crate::server::state::RefreshStatus::InProgressWait => {
            // No initial data available, return loading status
            tracing::debug!("🔄 Cache empty, returning loading status");
            return Ok(Json(crate::server::api::ApiResponse::loading()));
        }
        crate::server::state::RefreshStatus::InProgressServeStale => {
            tracing::debug!("🔄 Serving stale data while refresh in progress");
        }
        crate::server::state::RefreshStatus::NotNeeded => {
            tracing::debug!("✅ Cache is fresh");
        }
        crate::server::state::RefreshStatus::NeedsRefresh => {
            // This shouldn't happen since ensure_cache_available should have started refresh
            tracing::warn!("⚠️ Unexpected NeedsRefresh status after ensure_cache_available");
        }
    }

    // Get file existence info from cache (may be stale, but that's okay)
    let (file_existence_map, latest_image_dates) = {
        let cache = ctx.file_check_cache.read().unwrap();
        (
            cache.projects_with_files.clone(),
            cache.project_latest_image_dates.clone(),
        )
    };

    // Keep this request on the small project table. The background file scan
    // already walks the images and supplies the latest capture date above.
    let (projects, ambiguous_names) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        (
            db.get_projects_for_navigation().map_err(AppError::db)?,
            db.ambiguous_project_names().map_err(AppError::db)?,
        )
    };
    // Keep the established acquisition-backed scope. A scheduler pull seeds
    // newly changed projects into this map as known-but-missing, so they stay
    // visible without exposing planning-only projects.
    let projects = projects
        .into_iter()
        .filter(|(project, _)| file_existence_map.contains_key(&project.project.id))
        .collect::<Vec<_>>();
    let response: Vec<ProjectResponse> = projects
        .into_iter()
        .map(|(project_with_profile, state)| {
            let project = &project_with_profile.project;
            let display_name =
                display_project_name(&project.name, &project.profile_id, &ambiguous_names);

            ProjectResponse {
                id: project.id,
                profile_id: project.profile_id.clone(),
                profile_name: project_with_profile.profile_name.clone(),
                name: project.name.clone(),
                display_name,
                description: project.description.clone(),
                has_files: file_existence_map
                    .get(&project.id)
                    .copied()
                    .unwrap_or(false),
                state,
                latest_image_date: latest_image_dates.get(&project.id).copied(),
                guid: project.guid.clone(),
            }
        })
        .collect();

    tracing::debug!("📋 Returning {} projects", response.len());

    // Get current status for response
    let api_status = {
        let cache = ctx.file_check_cache.read().unwrap();
        crate::server::api::ApiRefreshStatus::from(cache.get_refresh_status())
    };

    Ok(Json(ApiResponse::success_with_status(response, api_status)))
}

pub async fn list_targets(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<Vec<TargetResponse>>>, AppError> {
    tracing::debug!("🎯 Listing targets for project {}", project_id);

    // Ensure cache is available (start refresh if needed)
    let refresh_status = ctx.ensure_cache_available();

    match refresh_status {
        crate::server::state::RefreshStatus::InProgressWait => {
            // No initial data available, return loading status
            tracing::debug!("🔄 Target cache empty, returning loading status");
            return Ok(Json(crate::server::api::ApiResponse::loading()));
        }
        crate::server::state::RefreshStatus::InProgressServeStale => {
            tracing::debug!("🔄 Serving stale target data while refresh in progress");
        }
        crate::server::state::RefreshStatus::NotNeeded => {
            tracing::debug!("✅ Target cache is fresh");
        }
        crate::server::state::RefreshStatus::NeedsRefresh => {
            // This shouldn't happen since ensure_cache_available should have started refresh
            tracing::warn!("⚠️ Unexpected NeedsRefresh status after ensure_cache_available");
        }
    }

    // Get file existence info from cache (may be stale, but that's okay)
    let file_existence_map: HashMap<i32, bool> = {
        let cache = ctx.file_check_cache.read().unwrap();
        cache.targets_with_files.clone()
    };

    // Get ALL targets from database (not just those with files)
    let targets = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        db.get_targets_with_images(project_id)
            .map_err(AppError::db)?
    };

    let response: Vec<TargetResponse> = targets
        .into_iter()
        .map(|(target, img_count, accepted, rejected)| TargetResponse {
            id: target.id,
            name: target.name,
            ra: target.ra,
            dec: target.dec,
            active: target.active,
            image_count: img_count,
            accepted_count: accepted,
            rejected_count: rejected,
            has_files: file_existence_map.get(&target.id).copied().unwrap_or(false),
        })
        .collect();

    tracing::debug!(
        "🎯 Returning {} targets for project {}",
        response.len(),
        project_id
    );

    // Get current status for response
    let api_status = {
        let cache = ctx.file_check_cache.read().unwrap();
        crate::server::api::ApiRefreshStatus::from(cache.get_refresh_status())
    };

    Ok(Json(ApiResponse::success_with_status(response, api_status)))
}

pub async fn list_all_targets(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<TargetNavigationResponse>>>, AppError> {
    tracing::debug!("🎯 Listing targets across all projects");

    // Start a cache refresh if needed, but do not hide the target rows while
    // the first file scan runs. Navigation itself needs only the target table.
    ctx.ensure_cache_available();

    let (file_existence_map, cache_ready): (HashMap<i32, bool>, bool) = {
        let cache = ctx.file_check_cache.read().unwrap();
        (cache.targets_with_files.clone(), cache.has_initial_data)
    };

    let targets = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        db.get_target_navigation().map_err(AppError::db)?
    };

    let response = targets
        .into_iter()
        .filter(|target| !cache_ready || file_existence_map.contains_key(&target.id))
        .map(|target| TargetNavigationResponse {
            id: target.id,
            project_id: target.project_id,
            name: target.name,
            active: target.active,
            has_files: file_existence_map.get(&target.id).copied().unwrap_or(false),
        })
        .collect();

    let api_status = {
        let cache = ctx.file_check_cache.read().unwrap();
        crate::server::api::ApiRefreshStatus::from(cache.get_refresh_status())
    };

    Ok(Json(ApiResponse::success_with_status(response, api_status)))
}

// Error handling
#[derive(Debug)]
pub enum AppError {
    NotFound,
    /// 404 that explains itself — for resources that legitimately disappear,
    /// where "Resource not found" would read as an auth failure.
    NotFoundMessage(String),
    DatabaseError(String),
    BadRequest(String),
    Conflict(String),
    Forbidden(String),
    InternalError(String),
    NotImplemented,
}

impl AppError {
    /// Wrap any database-layer failure, preserving its message —
    /// `map_err(AppError::db)`. Losing the underlying rusqlite error (as the
    /// old unit `DatabaseError` did) made lock contention ("database is
    /// locked") indistinguishable from corruption or I/O failures in the logs.
    pub(crate) fn db(err: impl std::fmt::Display) -> Self {
        AppError::DatabaseError(err.to_string())
    }

    /// What a person reads, for an error a job records rather than answers.
    pub(crate) fn message(&self) -> String {
        match self {
            AppError::NotFound => "Resource not found".into(),
            AppError::NotImplemented => "Not implemented".into(),
            AppError::DatabaseError(message) => format!("Database error: {message}"),
            AppError::NotFoundMessage(message)
            | AppError::BadRequest(message)
            | AppError::Conflict(message)
            | AppError::Forbidden(message)
            | AppError::InternalError(message) => message.clone(),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::NotFound => {
                tracing::warn!("🔍 Resource not found");
                (StatusCode::NOT_FOUND, "Resource not found")
            }
            AppError::NotFoundMessage(msg) => {
                tracing::warn!("🔍 Resource not found: {}", msg);
                return (
                    StatusCode::NOT_FOUND,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::DatabaseError(msg) => {
                tracing::error!("💾 Database error: {}", msg);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(format!("Database error: {}", msg))),
                )
                    .into_response();
            }
            AppError::BadRequest(msg) => {
                tracing::warn!("❌ Bad request: {}", msg);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::Conflict(msg) => {
                tracing::warn!("⚠️ Conflict: {}", msg);
                return (
                    StatusCode::CONFLICT,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::Forbidden(msg) => {
                tracing::warn!("🚫 Forbidden: {}", msg);
                return (
                    StatusCode::FORBIDDEN,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::InternalError(msg) => {
                tracing::error!("⚠️  Internal server error: {}", msg);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::NotImplemented => {
                tracing::debug!("🚧 Not implemented endpoint accessed");
                (StatusCode::NOT_IMPLEMENTED, "Not implemented yet")
            }
        };

        (
            status,
            Json(ApiResponse::<()>::error(error_message.to_string())),
        )
            .into_response()
    }
}

#[cfg(test)]
mod preview_cache_key_tests;

#[cfg(test)]
mod file_resolution_tests;
