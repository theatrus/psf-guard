use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::db::Database;
use crate::models::{GradingStatus, OverallDesiredStats, ProjectDesiredStats};
use crate::server::api::*;
use crate::server::database_context::DatabaseContext;
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

#[cfg(test)]
mod coordinate_format_tests {
    use super::format_coordinates;

    #[test]
    fn target_scheduler_ra_is_already_in_hours() {
        assert_eq!(
            format_coordinates(Some(10.5), Some(-20.25)).as_deref(),
            Some("RA 10h 30m 00.0s, Dec -20° 15' 00.0\"")
        );
    }
}

pub async fn get_server_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ServerInfo>>, AppError> {
    let info = ServerInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        cache_directory: state.cache_dir_root.clone(),
        allow_database_management: state.database_management_allowed(),
        banner: state.site_banner(),
    };

    Ok(Json(ApiResponse::success(info)))
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
    let guard = state.begin_interactive_job();
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

/// Download and install one hosted Seiza catalog package. This writes to the
/// configured catalog directory, so it uses the same trust gate as database
/// management. The work continues after the request returns.
pub async fn start_astrometry_catalog_install(
    State(state): State<Arc<AppState>>,
    Json(request): Json<crate::server::catalog_install::CatalogInstallRequest>,
) -> Result<Json<ApiResponse<crate::server::catalog_install::CatalogInstallStatus>>, AppError> {
    require_database_management_allowed(&state)?;
    let output_dir = state.astrometry.catalog_install_dir();
    let guard = state.begin_interactive_job();
    let status = state
        .catalog_install
        .start(request.preset, output_dir, guard);
    Ok(Json(ApiResponse::success(status)))
}

/// Header-only catalog association and embedded-WCS overlay geometry for one
/// image. This stays separate from image metadata so provenance, partial
/// capability, and later plate-solve results retain a typed contract.
pub async fn get_image_astrometry(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<crate::astrometry::AstrometryAnalysis>>, AppError> {
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx, image_id)?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_interactive_job();
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
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx, image_id)?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_interactive_job();
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
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx, image_id)?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_interactive_job();
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
    let (fits_path, expected_target) = resolve_astrometry_input(&ctx, image_id)?;
    let cache_dir = ctx.cache_dir_path.clone();
    let astrometry = Arc::clone(&state.astrometry);
    let guard = state.begin_interactive_job();
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

fn resolve_astrometry_input(
    ctx: &DatabaseContext,
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
    let fits_path = find_fits_file(ctx, &image, &target_name, &file_only)?;
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
        .map(|ctx| DatabaseSummary {
            id: ctx.id.clone(),
            name: ctx.name.clone(),
            database_path: ctx.database_path.clone(),
            image_directories: ctx.image_dirs.clone(),
            remote_image_upload: remote_image_upload_summary(ctx),
        })
        .collect();
    summaries.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(Json(ApiResponse::success(summaries)))
}

fn require_registry_path(state: &AppState) -> Result<std::path::PathBuf, AppError> {
    state.registry_path.read().unwrap().clone().ok_or_else(|| {
        AppError::BadRequest(
            "database registry persistence is not configured for this server".into(),
        )
    })
}

/// Gate for mutating endpoints on `/api/databases`. Returns 403 when the
/// server was launched without `--allow-database-management`. Anyone reachable
/// over the network could otherwise re-point, remove, or sync the user's
/// configured databases.
pub(super) fn require_database_management_allowed(state: &AppState) -> Result<(), AppError> {
    if state.database_management_allowed() {
        Ok(())
    } else {
        Err(AppError::Forbidden(
            "database management is disabled on this server. Restart the server \
            with --allow-database-management to enable runtime database changes and sync."
                .into(),
        ))
    }
}

fn summary_of(ctx: &crate::server::database_context::DatabaseContext) -> DatabaseSummary {
    DatabaseSummary {
        id: ctx.id.clone(),
        name: ctx.name.clone(),
        database_path: ctx.database_path.clone(),
        image_directories: ctx.image_dirs.clone(),
        remote_image_upload: remote_image_upload_summary(ctx),
    }
}

fn remote_image_upload_summary(
    ctx: &crate::server::database_context::DatabaseContext,
) -> RemoteImageUploadSummary {
    let config = ctx.remote_image_upload.as_ref();
    RemoteImageUploadSummary {
        enabled: ctx.remote_image_upload_dir.is_some(),
        image_directory: config
            .map(|config| config.image_dir.clone())
            .filter(|directory| !directory.is_empty()),
        token_configured: config.is_some_and(|config| config.token_is_configured()),
    }
}

/// `POST /api/databases/{db_id}/sync` — preview or run one safe scheduler
/// database sync. The path database is the local working copy. A pull reads
/// the peer and fills the local copy; a planning push reads the local copy and
/// updates planning settings in the peer.
pub async fn sync_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
    Json(req): Json<SchedulerSyncRequest>,
) -> Result<Json<ApiResponse<SchedulerSyncResponse>>, AppError> {
    let _apply_guard = if req.dry_run {
        None
    } else {
        Some(state.sync_apply_lock.lock().await)
    };
    let response = execute_scheduler_sync(&state, &db_id, req).await?;
    Ok(Json(ApiResponse::success(response)))
}

async fn execute_scheduler_sync(
    state: &Arc<AppState>,
    db_id: &str,
    req: SchedulerSyncRequest,
) -> Result<SchedulerSyncResponse, AppError> {
    execute_scheduler_sync_guarded(state, db_id, req, None, SyncGuardMode::None)
        .await
        .map(|(response, _)| response)
}

enum SyncGuardMode {
    None,
    Preview,
    Apply { destination_fingerprint: String },
}

#[derive(Debug)]
struct StaleSyncPreview;

impl std::fmt::Display for StaleSyncPreview {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("sync preview destination changed")
    }
}

impl std::error::Error for StaleSyncPreview {}

async fn execute_scheduler_sync_guarded(
    state: &Arc<AppState>,
    db_id: &str,
    req: SchedulerSyncRequest,
    source_override: Option<PathBuf>,
    guard_mode: SyncGuardMode,
) -> Result<(SchedulerSyncResponse, Option<String>), AppError> {
    use crate::commands::sync::{
        parse_status, sync_grades, sync_grades_in_transaction, sync_planning,
        sync_planning_in_transaction, sync_pull, sync_pull_in_transaction, PlanningOptions,
        PullOptions, SyncGradesOptions,
    };
    use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior};

    require_database_management_allowed(state)?;
    if db_id == req.peer_db_id {
        return Err(AppError::BadRequest(
            "Choose two different databases to sync.".into(),
        ));
    }

    let local = state.get_database(db_id).ok_or(AppError::NotFound)?;
    let peer = state
        .get_database(&req.peer_db_id)
        .ok_or(AppError::NotFound)?;
    let (source_ctx, destination_ctx) = match req.kind {
        SchedulerSyncKind::Pull => (Arc::clone(&peer), Arc::clone(&local)),
        SchedulerSyncKind::PushPlanning | SchedulerSyncKind::PushGrades => {
            (Arc::clone(&local), Arc::clone(&peer))
        }
    };
    let source_path = source_override.unwrap_or_else(|| PathBuf::from(&source_ctx.database_path));
    let destination_path = PathBuf::from(&destination_ctx.database_path);
    let source_id = source_ctx.id.clone();
    let destination_id = destination_ctx.id.clone();
    let fingerprint_queries = crate::server::sync_preview::fingerprint_queries(&req);
    let kind = req.kind;
    let dry_run = req.dry_run;
    let project = req.project;
    let target = req.target;
    let status_filter = if matches!(kind, SchedulerSyncKind::PushGrades) {
        req.status
            .as_deref()
            .map(parse_status)
            .transpose()
            .map_err(|error| AppError::BadRequest(error.to_string()))?
    } else {
        None
    };
    let reviewed_only = req.reviewed_only;
    let with_image_data = req.with_image_data.unwrap_or(true);

    let response = tokio::task::spawn_blocking(
        move || -> anyhow::Result<(SchedulerSyncResponse, Option<String>)> {
            let source_canon =
                std::fs::canonicalize(&source_path).unwrap_or_else(|_| source_path.clone());
            let destination_canon = std::fs::canonicalize(&destination_path)
                .unwrap_or_else(|_| destination_path.clone());
            anyhow::ensure!(
                source_canon != destination_canon,
                "The two configured entries point to the same database file."
            );

            let source =
                Connection::open_with_flags(&source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let destination =
                Connection::open_with_flags(&destination_path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;

            let run =
                |transaction: Option<&Transaction<'_>>| -> anyhow::Result<SchedulerSyncResponse> {
                    let response = match kind {
                        SchedulerSyncKind::Pull => {
                            let options = PullOptions {
                                dry_run,
                                with_image_data,
                                project_filter: project.clone(),
                            };
                            let summary = match transaction {
                                Some(transaction) => {
                                    sync_pull_in_transaction(&source, transaction, &options)?
                                }
                                None => sync_pull(&source, &destination, &options)?,
                            };
                            SchedulerSyncResponse {
                                kind,
                                dry_run,
                                source_db_id: source_id,
                                destination_db_id: destination_id,
                                exposuretemplate: (&summary.exposuretemplate).into(),
                                project: (&summary.project).into(),
                                ruleweight: (&summary.ruleweight).into(),
                                target: (&summary.target).into(),
                                exposureplan: (&summary.exposureplan).into(),
                                acquiredimage: Some((&summary.acquiredimage).into()),
                                imagedata: summary
                                    .imagedata_synced
                                    .then(|| (&summary.imagedata).into()),
                                grades: None,
                                grade_filled: summary.grade_filled,
                                grade_preserved: summary.grade_preserved,
                                imagedata_bytes: summary.imagedata_bytes,
                                total_inserted: summary.total_inserted(),
                                total_updated: summary.total_updated(),
                            }
                        }
                        SchedulerSyncKind::PushPlanning => {
                            let options = PlanningOptions {
                                dry_run,
                                project_filter: project.clone(),
                            };
                            let summary = match transaction {
                                Some(transaction) => {
                                    sync_planning_in_transaction(&source, transaction, &options)?
                                }
                                None => sync_planning(&source, &destination, &options)?,
                            };
                            SchedulerSyncResponse {
                                kind,
                                dry_run,
                                source_db_id: source_id,
                                destination_db_id: destination_id,
                                exposuretemplate: (&summary.exposuretemplate).into(),
                                project: (&summary.project).into(),
                                ruleweight: (&summary.ruleweight).into(),
                                target: (&summary.target).into(),
                                exposureplan: (&summary.exposureplan).into(),
                                acquiredimage: None,
                                imagedata: None,
                                grades: None,
                                grade_filled: 0,
                                grade_preserved: 0,
                                imagedata_bytes: 0,
                                total_inserted: summary.total_inserted(),
                                total_updated: summary.total_updated(),
                            }
                        }
                        SchedulerSyncKind::PushGrades => {
                            let options = SyncGradesOptions {
                                status_filter,
                                reviewed_only,
                                project_filter: project.clone(),
                                target_filter: target.clone(),
                                dry_run,
                            };
                            let summary = match transaction {
                                Some(transaction) => {
                                    sync_grades_in_transaction(&source, transaction, &options)?
                                }
                                None => sync_grades(&source, &destination, &options)?,
                            };
                            SchedulerSyncResponse {
                                kind,
                                dry_run,
                                source_db_id: source_id,
                                destination_db_id: destination_id,
                                exposuretemplate: SchedulerSyncTableCounts::default(),
                                project: SchedulerSyncTableCounts::default(),
                                ruleweight: SchedulerSyncTableCounts::default(),
                                target: SchedulerSyncTableCounts::default(),
                                exposureplan: SchedulerSyncTableCounts::default(),
                                acquiredimage: None,
                                imagedata: None,
                                grades: Some(SchedulerSyncGradeCounts {
                                    source_considered: summary.source_considered,
                                    source_no_guid: summary.source_no_guid,
                                    matched: summary.matched,
                                    changed: summary.changed,
                                    unchanged: summary.unchanged,
                                    unmatched_source: summary.unmatched_source,
                                    destination_only: summary.dest_only,
                                    duplicate_guids: summary.duplicate_guids,
                                    transitions: summary.transitions,
                                }),
                                grade_filled: 0,
                                grade_preserved: 0,
                                imagedata_bytes: 0,
                                total_inserted: 0,
                                total_updated: summary.changed,
                            }
                        }
                    };
                    Ok(response)
                };

            match guard_mode {
                SyncGuardMode::None => Ok((run(None)?, None)),
                SyncGuardMode::Preview => {
                    let transaction =
                        Transaction::new_unchecked(&destination, TransactionBehavior::Deferred)?;
                    let fingerprint = crate::server::sync_preview::connection_fingerprint(
                        &transaction,
                        &fingerprint_queries,
                    )?;
                    let response = run(Some(&transaction))?;
                    transaction.rollback()?;
                    Ok((response, Some(fingerprint)))
                }
                SyncGuardMode::Apply {
                    destination_fingerprint,
                } => {
                    let transaction =
                        Transaction::new_unchecked(&destination, TransactionBehavior::Immediate)?;
                    let fingerprint = crate::server::sync_preview::connection_fingerprint(
                        &transaction,
                        &fingerprint_queries,
                    )?;
                    if fingerprint != destination_fingerprint {
                        return Err(StaleSyncPreview.into());
                    }
                    let response = run(Some(&transaction))?;
                    transaction.commit()?;
                    Ok((response, None))
                }
            }
        },
    )
    .await
    .map_err(|error| AppError::InternalError(format!("scheduler sync task failed: {error}")))?
    .map_err(|error| {
        if error.downcast_ref::<StaleSyncPreview>().is_some() {
            AppError::Conflict(
                "This preview is stale because the destination database changed. Preview again."
                    .into(),
            )
        } else {
            AppError::BadRequest(format!("{error:#}"))
        }
    })?;

    if !dry_run {
        let _ = destination_ctx.ensure_cache_available();
    }
    Ok(response)
}

fn sync_endpoint_paths(
    state: &AppState,
    local_db_id: &str,
    request: &SchedulerSyncRequest,
) -> Result<(PathBuf, PathBuf), AppError> {
    if local_db_id == request.peer_db_id {
        return Err(AppError::BadRequest(
            "Choose two different databases to sync.".into(),
        ));
    }
    let local = state.get_database(local_db_id).ok_or(AppError::NotFound)?;
    let peer = state
        .get_database(&request.peer_db_id)
        .ok_or(AppError::NotFound)?;
    let (source, destination) = match request.kind {
        SchedulerSyncKind::Pull => (peer, local),
        SchedulerSyncKind::PushPlanning | SchedulerSyncKind::PushGrades => (local, peer),
    };
    Ok((
        PathBuf::from(&source.database_path),
        PathBuf::from(&destination.database_path),
    ))
}

/// Create a server-owned dry preview. Apply is a separate endpoint keyed by
/// the returned opaque preview ID.
pub async fn preview_sync_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
    Json(mut request): Json<SchedulerSyncRequest>,
) -> Result<Json<ApiResponse<SchedulerSyncPreviewResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    request.dry_run = true;
    let (source_path, _destination_path) = sync_endpoint_paths(&state, &db_id, &request)?;
    let source_snapshot_file = state
        .sync_previews
        .create_source_snapshot(&source_path)
        .map_err(|error| AppError::BadRequest(format!("{error:#}")))?;
    let source_snapshot_path = state
        .sync_previews
        .source_snapshot_path_for_file(&source_snapshot_file)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;
    let preview = execute_scheduler_sync_guarded(
        &state,
        &db_id,
        request.clone(),
        Some(source_snapshot_path),
        SyncGuardMode::Preview,
    )
    .await;
    let (result, destination_fingerprint) = match preview {
        Ok((result, Some(fingerprint))) => (result, fingerprint),
        Ok(_) => unreachable!("preview execution always returns a fingerprint"),
        Err(error) => {
            state
                .sync_previews
                .remove_source_snapshot(&source_snapshot_file);
            return Err(error);
        }
    };

    let record = state
        .sync_previews
        .store(
            db_id,
            request,
            source_snapshot_file.clone(),
            destination_fingerprint,
            result.clone(),
        )
        .map_err(|error| {
            state
                .sync_previews
                .remove_source_snapshot(&source_snapshot_file);
            AppError::InternalError(format!("saving scheduler sync preview: {error}"))
        })?;
    Ok(Json(ApiResponse::success(SchedulerSyncPreviewResponse {
        preview_id: record.id,
        created_at: record.created_at,
        expires_at: record.expires_at,
        result,
    })))
}

pub async fn get_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<SchedulerSyncPreviewResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let record = state
        .sync_previews
        .get(&preview_id)
        .map_err(|error| AppError::InternalError(format!("loading sync preview: {error}")))?
        .filter(|record| record.local_db_id == db_id)
        .ok_or(AppError::NotFound)?;
    Ok(Json(ApiResponse::success(SchedulerSyncPreviewResponse {
        preview_id: record.id,
        created_at: record.created_at,
        expires_at: record.expires_at,
        result: record.result,
    })))
}

pub async fn delete_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<bool>>, AppError> {
    require_database_management_allowed(&state)?;
    let deleted = state
        .sync_previews
        .discard(&preview_id, &db_id)
        .map_err(|error| AppError::InternalError(format!("deleting sync preview: {error}")))?;
    Ok(Json(ApiResponse::success(deleted)))
}

/// Apply one unexpired preview after proving that neither catalog changed.
pub async fn apply_sync_database_preview_route(
    State(state): State<Arc<AppState>>,
    Path((db_id, preview_id)): Path<(String, String)>,
) -> Result<Json<ApiResponse<SchedulerSyncResponse>>, AppError> {
    require_database_management_allowed(&state)?;
    let _apply_guard = state.sync_apply_lock.lock().await;
    let record = state
        .sync_previews
        .claim(&preview_id, &db_id)
        .map_err(|error| AppError::InternalError(format!("loading sync preview: {error}")))?
        .ok_or(AppError::NotFound)?;

    let source_snapshot = state
        .sync_previews
        .source_snapshot_path(&record)
        .map_err(|error| AppError::InternalError(format!("{error:#}")))?;

    let mut request = record.request;
    request.dry_run = false;
    let result = execute_scheduler_sync_guarded(
        &state,
        &db_id,
        request,
        Some(source_snapshot),
        SyncGuardMode::Apply {
            destination_fingerprint: record.destination_fingerprint,
        },
    )
    .await
    .map(|(result, _)| result);
    state
        .sync_previews
        .remove_source_snapshot(&record.source_snapshot_file);
    let result = result?;
    Ok(Json(ApiResponse::success(result)))
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
    let mut reg = DbRegistry::load_or_init(&registry_path)
        .map_err(|e| AppError::InternalError(format!("loading registry: {}", e)))?;

    let entry = reg
        .add(req.name, req.db_path, req.image_dirs, req.slug)
        .map_err(|e| AppError::BadRequest(e.to_string()))?
        .clone();

    // Open the connection now so persisting bad config can't outlive a failure.
    let ctx = Arc::new(
        DatabaseContext::new(
            entry.id.clone(),
            entry.name.clone(),
            entry.db_path.clone(),
            entry.image_dirs.clone(),
            entry.remote_image_upload.clone(),
            state.cache_dir_root.clone(),
        )
        .map_err(|e| AppError::BadRequest(format!("opening database: {}", e)))?,
    );

    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    state
        .databases
        .write()
        .unwrap()
        .insert(entry.id.clone(), ctx.clone());

    // Kick a background refresh for the newly-registered DB so file/dir caches
    // populate without the user having to refresh manually.
    let _ = ctx.ensure_cache_available();

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
    let mut reg = DbRegistry::load_or_init(&registry_path)
        .map_err(|e| AppError::InternalError(format!("loading registry: {}", e)))?;

    if reg.find(&db_id).is_none() {
        return Err(AppError::NotFound);
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
    if let Some(update) = req.remote_image_upload.as_ref() {
        let entry = reg
            .databases
            .iter_mut()
            .find(|entry| entry.id == new_id)
            .ok_or(AppError::InternalError(
                "registry update lost the entry".into(),
            ))?;
        apply_remote_image_upload_update(entry, update)?;
    }
    let entry = reg
        .find(&new_id)
        .ok_or(AppError::InternalError(
            "registry update lost the entry".into(),
        ))?
        .clone();

    // If the slug changed, move the on-disk cache directory so previously
    // generated previews carry over to the new identity. Failure is non-fatal:
    // worst case the cache is rebuilt under the new slug.
    if new_id != db_id {
        let old_dir = std::path::PathBuf::from(&state.cache_dir_root).join(&db_id);
        let new_dir = std::path::PathBuf::from(&state.cache_dir_root).join(&new_id);
        if old_dir.exists() {
            if let Err(e) = std::fs::rename(&old_dir, &new_dir) {
                tracing::warn!(
                    "Failed to rename cache dir {} -> {}: {} (old cache will be orphaned)",
                    old_dir.display(),
                    new_dir.display(),
                    e
                );
            } else {
                tracing::info!(
                    "Renamed cache dir {} -> {}",
                    old_dir.display(),
                    new_dir.display()
                );
            }
        }
    }

    let new_ctx = Arc::new(
        DatabaseContext::new(
            entry.id.clone(),
            entry.name.clone(),
            entry.db_path.clone(),
            entry.image_dirs.clone(),
            entry.remote_image_upload.clone(),
            state.cache_dir_root.clone(),
        )
        .map_err(|e| AppError::BadRequest(format!("opening database: {}", e)))?,
    );

    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    {
        let mut map = state.databases.write().unwrap();
        // If slug changed, remove the old entry.
        if new_id != db_id {
            map.remove(&db_id);
        }
        map.insert(new_id.clone(), new_ctx.clone());
    }

    let _ = new_ctx.ensure_cache_available();

    Ok(Json(ApiResponse::success(summary_of(&new_ctx))))
}

fn apply_remote_image_upload_update(
    entry: &mut crate::db_registry::DbEntry,
    update: &RemoteImageUploadUpdate,
) -> Result<(), AppError> {
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
    if config.enabled {
        config
            .validated_image_dir(&entry.image_dirs)
            .map_err(|error| AppError::BadRequest(error.to_string()))?;
    }
    entry.remote_image_upload = Some(config);
    Ok(())
}

/// `DELETE /api/databases/{db_id}` — drop the registered database. Returns
/// 200 with `{removed: true}` even on first-call (idempotent on the surface).
pub async fn remove_database_route(
    State(state): State<Arc<AppState>>,
    Path(db_id): Path<String>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    use crate::db_registry::DbRegistry;

    require_database_management_allowed(&state)?;
    let registry_path = require_registry_path(&state)?;
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

/// `GET /api/db/{db_id}/export` — stream the selected non-rejected lights as
/// an uncompressed (store-mode) zip, laid out exactly like the CLI export
/// (`<target>/LIGHT/<filter>/<basename>`). FITS doesn't compress, so store
/// mode streams at wire speed with no server-side staging. Read-only, so it
/// is not management-gated.
pub async fn export_archive_route(
    ctx: DbContext,
    Query(query): Query<ExportQuery>,
) -> Result<axum::response::Response, AppError> {
    use crate::commands::export::{plan_export, ExportOptions};
    use axum::body::Body;
    use axum::http::header;

    let options = ExportOptions {
        include_pending: query.include_pending,
        project_id: query.project_id,
        target_id: query.target_id,
        filter_name: query.filter_name.clone(),
        ..Default::default()
    };

    // Plan on a blocking thread: it queries the DB and walks the image dirs.
    // Use a DEDICATED read-only connection — the walk can take tens of
    // seconds on network storage, and holding the shared request-connection
    // mutex for that long would block every other API call on this DB
    // (same rule as the import job and the background file-check refresh).
    let plan_ctx = ctx.0.clone();
    let plan = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open_with_flags(
            &plan_ctx.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", plan_ctx.database_path))?;
        plan_export(&conn, &plan_ctx.image_dirs, &options)
    })
    .await
    .map_err(|e| AppError::InternalError(format!("export planning task: {e}")))?
    .map_err(|e| AppError::InternalError(format!("planning export: {e}")))?;

    if plan.items.is_empty() {
        return Err(AppError::BadRequest(format!(
            "nothing to export ({} matching rows had missing files)",
            plan.missing.len()
        )));
    }

    let filename = format!(
        "psf-guard-export-{}{}.zip",
        ctx.id,
        query
            .target_id
            .map(|t| format!("-target{}", t))
            .unwrap_or_default()
    );
    let total_files = plan.items.len();
    let db_id = ctx.id.clone();

    // Zip writer feeds one end of a duplex pipe; the response body streams
    // the other. A mid-stream file error truncates the download (logged) —
    // the client sees a corrupt archive rather than a silent partial success.
    let (writer, reader) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        use async_zip::tokio::write::ZipFileWriter;
        use async_zip::{Compression, ZipEntryBuilder};

        let mut zip = ZipFileWriter::with_tokio(writer);
        for item in &plan.items {
            let entry_name = item.relative_dest.to_string_lossy().replace('\\', "/");
            let bytes = match tokio::fs::read(&item.source).await {
                Ok(bytes) => bytes,
                Err(e) => {
                    tracing::warn!(
                        "📦 export db={}: aborting stream, {} unreadable: {}",
                        db_id,
                        item.source.display(),
                        e
                    );
                    return;
                }
            };
            let entry = ZipEntryBuilder::new(entry_name.into(), Compression::Stored);
            if let Err(e) = zip.write_entry_whole(entry, &bytes).await {
                tracing::warn!("📦 export db={}: zip write failed: {}", db_id, e);
                return;
            }
        }
        if let Err(e) = zip.close().await {
            tracing::warn!("📦 export db={}: zip finalize failed: {}", db_id, e);
        } else {
            tracing::info!("📦 export db={}: streamed {} file(s)", db_id, total_files);
        }
    });

    let body = Body::from_stream(tokio_util::io::ReaderStream::new(reader));
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "application/zip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .body(body)
        .map_err(|e| AppError::InternalError(format!("building response: {e}")))
}

/// `POST /api/db/{db_id}/export/local` — run the folder export on the
/// server's own filesystem (copy or hardlink), for desktop/Tauri mode where
/// server and user share a machine. Same selection and layout as the CLI
/// `export` command and the zip stream. Management-gated: a remote client
/// could otherwise write files to arbitrary server paths.
pub async fn export_local_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<LocalExportRequest>,
) -> Result<Json<ApiResponse<crate::commands::export::ExportSummary>>, AppError> {
    use crate::commands::export::{execute_plan, plan_export, ExportOptions};

    require_database_management_allowed(&state)?;
    let dest = req.dest.trim().to_string();
    if dest.is_empty() {
        return Err(AppError::BadRequest("dest must not be empty".into()));
    }

    let options = ExportOptions {
        include_pending: req.include_pending,
        project_id: req.project_id,
        target_id: req.target_id,
        filter_name: req.filter_name.clone(),
        ..Default::default()
    };
    let link = req.link.unwrap_or(true);
    let dry_run = req.dry_run;

    // Plan + place on a blocking thread with a dedicated read-only
    // connection (same rule as the zip stream: never hold the shared
    // request-connection mutex through a directory walk).
    let plan_ctx = ctx.0.clone();
    let summary = tokio::task::spawn_blocking(move || {
        let conn = rusqlite::Connection::open_with_flags(
            &plan_ctx.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|e| anyhow::anyhow!("opening {}: {e}", plan_ctx.database_path))?;
        let plan = plan_export(&conn, &plan_ctx.image_dirs, &options)?;
        Ok::<_, anyhow::Error>(execute_plan(
            &plan,
            std::path::Path::new(&dest),
            link,
            dry_run,
        ))
    })
    .await
    .map_err(|e| AppError::InternalError(format!("export task: {e}")))?
    .map_err(|e| AppError::InternalError(format!("export: {e}")))?;

    tracing::info!(
        "📤 Local export db={}: planned={} copied={} linked={} skipped={} missing={} errors={}{}",
        ctx.id,
        summary.planned,
        summary.copied,
        summary.linked,
        summary.skipped_existing,
        summary.missing,
        summary.errors,
        if dry_run { " (dry-run)" } else { "" }
    );
    Ok(Json(ApiResponse::success(summary)))
}

/// `PUT /api/db/{db_id}/projects/{project_id}` — update scheduler fields.
pub async fn update_project_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
    Json(req): Json<UpdateProjectRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    require_database_management_allowed(&state)?;
    crate::server::scheduler::update_project(ctx, project_id, req)?;
    Ok(Json(ApiResponse::success(
        serde_json::json!({ "updated": true }),
    )))
}

/// `PUT /api/db/{db_id}/targets/{target_id}` — rename a target and/or move it
/// to another project (same profile; images follow the target).
pub async fn update_target_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, target_id)): Path<(String, i32)>,
    Json(req): Json<UpdateTargetRequest>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    require_database_management_allowed(&state)?;
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
    require_database_management_allowed(&state)?;
    let (targets_moved, images_moved) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        Database::new(&conn)
            .merge_projects(project_id, req.into_project_id)
            .map_err(|e| AppError::BadRequest(e.to_string()))?
    };
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
        DatabaseContext::new(
            entry.id.clone(),
            entry.name.clone(),
            entry.db_path.clone(),
            entry.image_dirs.clone(),
            entry.remote_image_upload.clone(),
            state.cache_dir_root.clone(),
        )
        .map_err(|e| AppError::InternalError(format!("opening new database: {}", e)))?,
    );

    reg.save(&registry_path)
        .map_err(|e| AppError::InternalError(format!("persisting registry: {}", e)))?;

    state
        .databases
        .write()
        .unwrap()
        .insert(entry.id.clone(), ctx.clone());

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
    );

    Ok(Json(ApiResponse::success(CreateDatabaseResponse {
        database: summary_of(&ctx),
        import: crate::server::import_job::progress_snapshot(&ctx.import_job),
    })))
}

/// `POST /api/db/{db_id}/import` — start a background FITS import into an
/// existing database. One import runs per database at a time.
pub async fn start_import_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<ImportRequest>,
) -> Result<Json<ApiResponse<ImportStatusResponse>>, AppError> {
    require_database_management_allowed(&state)?;

    let dirs = match req.image_dirs {
        Some(dirs) if !dirs.is_empty() => dirs,
        _ => ctx.image_dirs.clone(),
    };
    if dirs.is_empty() {
        return Err(AppError::BadRequest(
            "no image directories: pass image_dirs or configure them on the database".into(),
        ));
    }
    for dir in &dirs {
        if !std::path::Path::new(dir).is_dir() {
            return Err(AppError::BadRequest(format!(
                "image directory does not exist: {}",
                dir
            )));
        }
    }

    let options = crate::commands::import::ImportOptions {
        time_gap_days: req
            .time_gap_days
            .unwrap_or(crate::commands::import::grouping::DEFAULT_TIME_GAP_DAYS),
        profile_id: req.profile_id,
        dry_run: req.dry_run,
        attach_existing: req.attach_existing.unwrap_or(true),
        match_radius_deg: req
            .match_radius_deg
            .unwrap_or(crate::commands::import::DEFAULT_MATCH_RADIUS_DEG),
    };
    let started = spawn_import_job(
        &state,
        ctx.0.clone(),
        dirs,
        options,
        req.backfill.unwrap_or(false),
    );
    Ok(Json(ApiResponse::success(ImportStatusResponse {
        started,
        progress: crate::server::import_job::progress_snapshot(&ctx.import_job),
    })))
}

/// `GET /api/db/{db_id}/import` — import job progress (1s poll).
pub async fn get_import_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<ImportStatusResponse>>, AppError> {
    let progress = crate::server::import_job::progress_snapshot(&ctx.import_job);
    Ok(Json(ApiResponse::success(ImportStatusResponse {
        started: progress.running,
        progress,
    })))
}

/// Filesystem-safe stem for a managed database file, derived from the
/// user-facing name (`My Rig!` → `my-rig`).
fn sanitize_file_stem(name: &str) -> String {
    let mut stem: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while stem.contains("--") {
        stem = stem.replace("--", "-");
    }
    let stem = stem.trim_matches('-').to_string();
    if stem.is_empty() {
        "database".to_string()
    } else {
        stem
    }
}

/// First non-existing `<base>/<stem>[-N].sqlite`.
fn unique_db_file(base: &std::path::Path, stem: &str) -> std::path::PathBuf {
    let candidate = base.join(format!("{stem}.sqlite"));
    if !candidate.exists() {
        return candidate;
    }
    for n in 2.. {
        let candidate = base.join(format!("{stem}-{n}.sqlite"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

/// Launch the singleton import job for one database: header scan → one-shot
/// import transaction. Optional quality work starts afterwards with the
/// background worker budget and does not delay import completion.
/// Returns false when a job is already running for this database.
fn spawn_import_job(
    state: &Arc<AppState>,
    ctx: Arc<DatabaseContext>,
    dirs: Vec<String>,
    options: crate::commands::import::ImportOptions,
    backfill: bool,
) -> bool {
    use crate::server::import_job as job;

    if !job::try_begin(&ctx.import_job, dirs.clone()) {
        return false;
    }
    tracing::info!(
        "📥 Import started for db={} ({} director{})",
        ctx.id,
        dirs.len(),
        if dirs.len() == 1 { "y" } else { "ies" }
    );

    let state = state.clone();
    tokio::spawn(async move {
        let job_store = ctx.import_job.clone();
        let _import_guard = ctx.image_import_mutex.lock().await;

        let blocking_ctx = ctx.clone();
        let blocking_options = options.clone();
        let dir_paths: Vec<std::path::PathBuf> =
            dirs.iter().map(std::path::PathBuf::from).collect();
        let import_result = tokio::task::spawn_blocking(move || {
            run_import_blocking(&blocking_ctx, &dir_paths, &blocking_options)
        })
        .await;

        let outcome = match import_result {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(e)) => {
                tracing::warn!("📥 Import for db={} failed: {:#}", ctx.id, e);
                job::finish(&job_store, Some(format!("{:#}", e)));
                return;
            }
            Err(join_err) => {
                tracing::error!("📥 Import task for db={} panicked: {}", ctx.id, join_err);
                job::finish(
                    &job_store,
                    Some(format!("import task panicked: {join_err}")),
                );
                return;
            }
        };

        tracing::info!(
            "📥 Import for db={} done: {} imported, {} projects, {} targets{}",
            ctx.id,
            outcome.imported,
            outcome.projects_created,
            outcome.targets_created,
            if outcome.dry_run { " (dry-run)" } else { "" }
        );
        let mut target_ids = outcome.created_target_ids.clone();
        for id in &outcome.attached_target_ids {
            if !target_ids.contains(id) {
                target_ids.push(*id);
            }
        }
        job::complete_import(&job_store, outcome);

        // New rows reference files the DB-based file cache hasn't seen; kick
        // the normal background refresh so the UI resolves them promptly.
        let _ = ctx.ensure_cache_available();

        // Quality analysis is a general database maintenance job, not an
        // import stage. An opt-in import only queues the changed targets.
        if backfill && !target_ids.is_empty() {
            spawn_quality_backfill(&state, ctx.clone(), target_ids, false);
        }
    });
    true
}

/// Stages 1+2 of the import job, on a blocking thread: collect files, scan
/// headers (with progress), then run the single import transaction on a
/// dedicated connection (never the shared request connection).
fn run_import_blocking(
    ctx: &DatabaseContext,
    dirs: &[std::path::PathBuf],
    options: &crate::commands::import::ImportOptions,
) -> anyhow::Result<crate::commands::import::ImportOutcome> {
    use crate::commands::import as imp;
    use crate::server::import_job as job;
    use rayon::prelude::*;

    let files = imp::collect_fits_files(dirs)?;
    job::set_scan_totals(&ctx.import_job, files.len(), 0);

    let store = ctx.import_job.clone();
    let frames: Vec<imp::headers::FrameMeta> = files
        .par_iter()
        .map(|path| {
            let meta = imp::headers::read_frame_meta(path);
            store.write().unwrap().progress.scanned_files += 1;
            meta
        })
        .collect();

    job::set_stage(&ctx.import_job, "importing");
    let mut conn = rusqlite::Connection::open_with_flags(
        &ctx.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| anyhow::anyhow!("opening {} for import: {}", ctx.database_path, e))?;
    imp::import_frames(&mut conn, frames, options)
}

/// Run (or wait for) the quality scan for one database target. The scan is a
/// per-DB singleton, so this retries while a previous target's scan is still
/// running, then waits for its own scan to complete.
async fn run_quality_scan_for_target(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    target_id: i32,
    force: bool,
) {
    use crate::server::spatial_scan;

    let poll = tokio::time::Duration::from_millis(1500);
    loop {
        let response = start_spatial_scan_with_priority(
            State(state.clone()),
            DbContext(ctx.clone()),
            Json(SpatialScanRequest {
                target_id,
                filter_name: None,
                force: false,
                force_spatial: force,
                force_astrometry: force,
                force_satellites: false,
            }),
            crate::concurrency::Priority::Background,
            false,
        )
        .await;
        match response {
            Ok(Json(body)) => {
                let Some(data) = body.data else { return };
                if data.started {
                    break; // our scan launched; wait for it below
                }
                if !data.progress.running {
                    return; // nothing to compute for this target
                }
                // Another scan (previous target) still running — wait, retry.
            }
            Err(e) => {
                tracing::warn!(
                    "📥 Backfill scan for db={} target={} not started: {:?}",
                    ctx.id,
                    target_id,
                    e
                );
                return;
            }
        }
        tokio::time::sleep(poll).await;
    }

    loop {
        tokio::time::sleep(poll).await;
        let (progress, _) = spatial_scan::progress_snapshot(&ctx.spatial_metrics);
        if !progress.running {
            break;
        }
    }
}

fn spawn_quality_backfill(
    state: &Arc<AppState>,
    ctx: Arc<DatabaseContext>,
    target_ids: Vec<i32>,
    force: bool,
) -> bool {
    use crate::server::quality_backfill as job;

    if !job::try_begin(&ctx.quality_backfill, force, target_ids.len()) {
        return false;
    }
    if target_ids.is_empty() {
        return true;
    }

    let state = Arc::clone(state);
    tokio::spawn(async move {
        tracing::info!(
            "📐 Database quality {} started for db={} ({} targets)",
            if force { "rescan" } else { "backfill" },
            ctx.id,
            target_ids.len()
        );
        for target_id in target_ids {
            job::begin_target(&ctx.quality_backfill, target_id);
            run_quality_scan_for_target(&state, &ctx, target_id, force).await;
            job::finish_target(&ctx.quality_backfill);
        }
        job::finish(&ctx.quality_backfill);
        tracing::info!("📐 Database quality work finished for db={}", ctx.id);
    });
    true
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
    let started = spawn_quality_backfill(&state, ctx.0.clone(), target_ids, req.force);
    Ok(Json(ApiResponse::success(QualityBackfillStatusResponse {
        started,
        progress: crate::server::quality_backfill::snapshot(&ctx.quality_backfill),
    })))
}

pub async fn get_quality_backfill_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<QualityBackfillStatusResponse>>, AppError> {
    let progress = crate::server::quality_backfill::snapshot(&ctx.quality_backfill);
    Ok(Json(ApiResponse::success(QualityBackfillStatusResponse {
        started: progress.running,
        progress,
    })))
}

pub async fn refresh_file_cache(
    ctx: DbContext,
) -> Result<Json<ApiResponse<FileCheckResponse>>, AppError> {
    tracing::info!("🔄 Manual cache refresh requested");

    // Check current refresh status
    let refresh_status = ctx.ensure_cache_available();

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

            // Now start refresh
            let refresh_status = ctx.ensure_cache_available();
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
    let file_existence_map: HashMap<i32, bool> = {
        let cache = ctx.file_check_cache.read().unwrap();
        cache.projects_with_files.clone()
    };

    // Get ALL projects with profile info from database (not just those with files)
    let (projects, profile_count) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let projects = db
            .get_projects_with_images_and_profile_info()
            .map_err(AppError::db)?;
        let profile_count = db.get_profile_count().map_err(AppError::db)?;

        (projects, profile_count)
    };

    let show_profile = profile_count > 1;

    let response: Vec<ProjectResponse> = projects
        .into_iter()
        .map(|project_with_profile| {
            let project = &project_with_profile.project;
            let display_name = if show_profile {
                format!("{} → {}", project_with_profile.profile_name, project.name)
            } else {
                project.name.clone()
            };

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

pub async fn get_images(
    ctx: DbContext,
    Query(params): Query<ImageQuery>,
) -> Result<Json<ApiResponse<Vec<ImageResponse>>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    // Get profile count to determine display format
    let profile_count = db.get_profile_count().map_err(AppError::db)?;
    let show_profile = profile_count > 1;

    // Convert status string to GradingStatus enum
    let status_filter = params.status.as_ref().and_then(|s| match s.as_str() {
        "pending" => Some(GradingStatus::Pending),
        "accepted" => Some(GradingStatus::Accepted),
        "rejected" => Some(GradingStatus::Rejected),
        _ => None,
    });

    let offset = params.offset.unwrap_or(0).max(0) as usize;
    let limit = params.limit.unwrap_or(100).max(0) as usize;
    let images = db
        .query_images_scoped(
            status_filter,
            params.project_id,
            params.target_id,
            Some(limit),
            offset,
        )
        .map_err(AppError::db)?;

    let response: Vec<ImageResponse> = images
        .into_iter()
        .map(|(img, proj_name, target_name)| {
            let metadata: serde_json::Value = serde_json::from_str(&img.metadata)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

            // Create display name - we need the profile_id to do this properly
            let project_display_name = match img.profile_id.as_ref() {
                Some(profile_id) if show_profile => format!("{} → {}", profile_id, proj_name),
                _ => proj_name.clone(),
            };

            ImageResponse {
                id: img.id,
                project_id: img.project_id,
                project_name: proj_name,
                project_display_name,
                target_id: img.target_id,
                target_name,
                acquired_date: img.acquired_date,
                filter_name: img.filter_name,
                grading_status: img.grading_status,
                reject_reason: img.reject_reason,
                metadata,
                filesystem_path: None, // Not calculated for bulk operations for performance
            }
        })
        .collect();

    Ok(Json(ApiResponse::success(response)))
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ImageResponse>>, AppError> {
    use crate::image_analysis::FitsImage;

    // Get image data from database first (before any async operations)
    let (image, proj_name, target_name, mut metadata, show_profile) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        // Get profile count to determine display format
        let profile_count = db.get_profile_count().map_err(AppError::db)?;
        let show_profile = profile_count > 1;

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get project and target names
        let scoped_images = db
            .query_images_scoped(None, Some(image.project_id), Some(image.target_id), None, 0)
            .map_err(AppError::db)?;

        let (_, proj_name, target_name) = scoped_images
            .into_iter()
            .find(|(img, _, _)| img.id == image_id)
            .ok_or(AppError::NotFound)?;

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        (image, proj_name, target_name, metadata, show_profile)
    }; // Database connection is dropped here

    // Now we can do async operations
    let stats_cache_filename = format!(
        "stats_{}_{}_{}_{}.json",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0)
    );
    let stats_cache_path = ctx.get_cache_path("stats", &stats_cache_filename);

    // Ensure cache directory exists
    if let Some(parent) = stats_cache_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    // Try to resolve the filesystem path for the FITS file
    let filesystem_path = metadata["FileName"].as_str().and_then(|filename| {
        filename
            .split(&['\\', '/'][..])
            .next_back()
            .map(|file_only| find_fits_file(&ctx, &image, &target_name, file_only))
    });

    let resolved_fits_path = filesystem_path
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let filesystem_path_string = resolved_fits_path.map(|p| p.to_string_lossy().to_string());

    // Check if statistics are already cached
    let fits_stats = if tokio::fs::metadata(&stats_cache_path).await.is_ok() {
        // Load from cache
        match tokio::fs::read_to_string(&stats_cache_path).await {
            Ok(cached_data) => serde_json::from_str::<serde_json::Value>(&cached_data).ok(),
            Err(_) => None,
        }
    } else if let Some(fits_path) = resolved_fits_path {
        // Calculate statistics from FITS file
        if let Ok(fits) = FitsImage::from_file(fits_path) {
            let stats = fits.calculate_basic_statistics();

            // Extract temperature and camera model from FITS headers
            let temperature = FitsImage::extract_temperature(fits_path);
            let camera_model = FitsImage::extract_camera_model(fits_path);

            let mut stats_json = serde_json::json!({
                "Min": stats.min,
                "Max": stats.max,
                "Mean": stats.mean,
                "Median": stats.median,
                "StdDev": stats.std_dev,
                "Mad": stats.mad
            });

            // Add temperature if available
            if let Some(temp) = temperature {
                stats_json["Temperature"] = serde_json::json!(temp);
            }

            // Add camera model if available
            if let Some(camera) = camera_model {
                stats_json["Camera"] = serde_json::json!(camera);
            }

            // Cache the statistics
            if let Ok(cached_data) = serde_json::to_string(&stats_json) {
                let _ = tokio::fs::write(&stats_cache_path, cached_data).await;
            }

            Some(stats_json)
        } else {
            None
        }
    } else {
        None
    };

    // Merge statistics into metadata if available
    if let (Some(stats), Some(metadata_obj)) = (fits_stats, metadata.as_object_mut())
        && let Some(stats_obj) = stats.as_object()
    {
        for (key, value) in stats_obj {
            metadata_obj.insert(key.clone(), value.clone());
        }
    }

    // Create display name
    let project_display_name = match image.profile_id.as_ref() {
        Some(profile_id) if show_profile => format!("{} → {}", profile_id, proj_name),
        _ => proj_name.clone(),
    };

    let response = ImageResponse {
        id: image.id,
        project_id: image.project_id,
        project_name: proj_name,
        project_display_name,
        target_id: image.target_id,
        target_name,
        acquired_date: image.acquired_date,
        filter_name: image.filter_name,
        grading_status: image.grading_status,
        reject_reason: image.reject_reason,
        metadata,
        filesystem_path: filesystem_path_string,
    };

    Ok(Json(ApiResponse::success(response)))
}

pub async fn update_image_grade(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Json(request): Json<UpdateGradeRequest>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    let status = match request.status.as_str() {
        "pending" => GradingStatus::Pending,
        "accepted" => GradingStatus::Accepted,
        "rejected" => GradingStatus::Rejected,
        _ => return Err(AppError::BadRequest("Invalid status".to_string())),
    };

    db.update_grading_status(image_id, status, request.reason.as_deref())
        .map_err(AppError::db)?;

    Ok(Json(ApiResponse::success(())))
}

/// Shared DB lookup for the image handlers: the acquired-image row, the FITS
/// basename (from `metadata.FileName`), and the target name.
fn resolve_image_meta(
    ctx: &DatabaseContext,
    image_id: i32,
) -> Result<(crate::models::AcquiredImage, String, String), AppError> {
    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    let image = db
        .get_images_by_ids(&[image_id])
        .map_err(AppError::db)?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;

    let target = db
        .get_targets_by_ids(&[image.target_id])
        .map_err(AppError::db)?
        .into_iter()
        .next()
        .ok_or(AppError::NotFound)?;
    let target_name = target.name.clone();

    let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
        .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;
    let filename = metadata["FileName"]
        .as_str()
        .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;
    let file_only = filename
        .split(&['\\', '/'][..])
        .next_back()
        .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
        .to_string();

    Ok((image, file_only, target_name))
}

/// Cache key for a stretched preview PNG. Must stay identical between the
/// preview handler, the status endpoint, and the pre-generation path so all
/// three address the same file.
fn preview_cache_key(
    image: &crate::models::AcquiredImage,
    file_only: &str,
    size: &str,
    stretch: bool,
    midtone: f64,
    shadow: f64,
) -> String {
    format!(
        "{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image.id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        if stretch { "stretch" } else { "linear" },
        (midtone * 10000.0) as i32,
        (shadow * 10000.0) as i32,
    )
}

/// Cache key for an annotated (star-marked) PNG. Same stability requirement.
fn annotated_cache_key(
    image: &crate::models::AcquiredImage,
    file_only: &str,
    size: &str,
    max_stars: usize,
) -> String {
    format!(
        "annotated_{}_{}_{}_{}_{}_{}_{}",
        image.id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        max_stars,
    )
}

/// Resolve the on-disk cache path for a preview/annotated artifact, creating
/// the category dir. `category` is `"previews"` or `"annotated"`.
fn artifact_cache_path(
    ctx: &DatabaseContext,
    category: &str,
    key: &str,
) -> Result<PathBuf, AppError> {
    let cm = crate::server::cache::CacheManager::new(PathBuf::from(&ctx.cache_dir));
    cm.ensure_category_dir(category)
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    Ok(cm.get_cached_path(category, key, "png"))
}

/// Serve a cached PNG from disk.
async fn serve_cached_png(cache_path: &std::path::Path) -> Result<Response, AppError> {
    let buffer = tokio::fs::read(cache_path)
        .await
        .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;
    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "max-age=86400"),
        ],
        buffer,
    )
        .into_response())
}

/// The immediate "not ready — poll for it" response on a cache miss. `<img>`
/// treats the non-image body as an error and the frontend then batch-polls the
/// generation-status endpoint.
fn generating_response() -> Response {
    (
        StatusCode::ACCEPTED,
        [(CACHE_CONTROL, "no-store")],
        Json(ApiResponse::success(
            crate::server::preview_queue::GenerationStatus {
                state: crate::server::preview_queue::GenerationState::Generating,
                error: None,
            },
        )),
    )
        .into_response()
}

// Image preview endpoint. Cache hit → 200 PNG; miss → enqueue on the bounded
// interactive queue and return 202 (never generates inside the request).
#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image_preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(options): Query<PreviewOptions>,
) -> Result<Response, AppError> {
    let size = options.size.as_deref().unwrap_or("screen");
    let stretch = options.stretch.unwrap_or(true);
    let midtone = options.midtone.unwrap_or(0.2);
    let shadow = options.shadow.unwrap_or(-2.8);

    let (image, file_only, target_name) = resolve_image_meta(&ctx, image_id)?;
    let cache_key = preview_cache_key(&image, &file_only, size, stretch, midtone, shadow);
    let cache_path = artifact_cache_path(&ctx, "previews", &cache_key)?;

    if cache_path.exists() {
        return serve_cached_png(&cache_path).await;
    }

    // Miss: resolve the source (404 if truly missing), hand generation to the
    // bounded interactive queue, and tell the client to poll.
    let fits_path = find_fits_file(&ctx, &image, &target_name, &file_only)?;
    state.enqueue_preview(crate::server::preview_queue::GenJob {
        fits_path,
        cache_path,
        kind: crate::server::preview_queue::GenKind::Preview {
            midtone,
            shadow,
            max_dimensions: crate::server::preview_queue::max_dimensions_for_size(size),
        },
    });
    Ok(generating_response())
}

// Helper function to find FITS file
pub fn find_fits_file(
    ctx: &DatabaseContext,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    use crate::commands::filter_rejected::get_possible_paths;

    tracing::debug!(
        "🔍 find_fits_file called for image_id={}, filename={}, target={}, base_dirs={:?}",
        image.id,
        filename,
        target_name,
        ctx.image_dirs
    );

    // Extract date from acquired_date
    let acquired_date = image
        .acquired_date
        .and_then(|d| chrono::DateTime::from_timestamp(d, 0))
        .ok_or_else(|| {
            tracing::error!(
                "❌ Invalid date for image {}: {:?}",
                image.id,
                image.acquired_date
            );
            AppError::BadRequest("Invalid date".to_string())
        })?;

    let date_str = acquired_date.format("%Y-%m-%d").to_string();
    tracing::debug!("📅 Date string for image {}: {}", image.id, date_str);

    // Try to find the file in different possible locations across all directories
    let mut all_possible_paths = Vec::new();
    for base_dir in &ctx.image_dirs {
        let paths = get_possible_paths(base_dir, &date_str, target_name, filename);
        all_possible_paths.extend(paths);
    }

    tracing::debug!(
        "🔎 Checking {} possible paths for image {} across {} directories",
        all_possible_paths.len(),
        image.id,
        ctx.image_dirs.len()
    );

    // Verify all base directories exist (they were checked during startup)
    for base_dir in &ctx.image_dirs {
        tracing::debug!("✅ Base directory exists: {}", base_dir);
    }

    for (idx, path) in all_possible_paths.iter().enumerate() {
        tracing::debug!(
            "  📁 Path {}: {:?} (exists: {})",
            idx + 1,
            path,
            path.exists()
        );
        if path.exists() {
            tracing::info!("✅ Found file at path {}: {:?}", idx + 1, path);
            return Ok(path.clone());
        }
    }

    tracing::debug!(
        "❌ File not found in standard paths for image {}, trying directory tree cache lookup",
        image.id
    );

    // Try directory tree cache lookup as fallback
    let search_start = std::time::Instant::now();
    let directory_tree = ctx.get_directory_tree().map_err(|e| {
        tracing::error!("Failed to get directory tree cache: {}", e);
        AppError::InternalError("Directory cache error".to_string())
    })?;

    tracing::debug!(
        "🌳 Directory tree cache has {} total files, {} unique filenames",
        directory_tree.stats().total_files,
        directory_tree.stats().unique_filenames
    );

    if let Some(first_path) = directory_tree.find_file_first(filename) {
        tracing::debug!(
            "🔍 Found first match in directory tree cache for {}",
            filename
        );

        if first_path.exists() {
            tracing::info!(
                "✅ Found file via directory tree cache in {:?}: {:?}",
                search_start.elapsed(),
                first_path
            );
            return Ok(first_path.clone());
        } else {
            tracing::warn!(
                "❌ First cached path is stale for {}: {:?}",
                filename,
                first_path
            );
        }
    } else {
        tracing::debug!(
            "🔍 No matches in directory tree cache for filename: {}",
            filename
        );
    }

    tracing::warn!(
        "❌ File not found in directory tree cache after {:?} for image {} ({})",
        search_start.elapsed(),
        image.id,
        filename
    );
    Err(AppError::NotFound)
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image_stars(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<StarDetectionResponse>>, AppError> {
    use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
    use crate::image_analysis::FitsImage;
    use crate::psf_fitting::PSFType;
    use crate::server::cache::CacheManager;

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(AppError::db)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    // Create comprehensive cache key for star detection results
    let cache_key = format!(
        "stars_{}_{}_{}_{}_{}",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_")
    );
    let cache_manager = CacheManager::new(PathBuf::from(&ctx.cache_dir));
    cache_manager
        .ensure_category_dir("stars")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("stars", &cache_key, "json");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        // Read from cache
        let cached_data = tokio::fs::read_to_string(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let response: StarDetectionResponse = serde_json::from_str(&cached_data)
            .map_err(|_| AppError::InternalError("Invalid cached data".to_string()))?;

        return Ok(Json(ApiResponse::success(response)));
    }

    // Find FITS file path first (this is fast)
    let fits_path = find_fits_file(&ctx, &image, &target_name, &file_only)?;

    // Move expensive operations to spawn_blocking
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let (stars, detected_count, average_hfr, average_fwhm) =
        tokio::task::spawn_blocking(move || {
            // Load FITS file
            let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))?;

            // Run star detection
            let params = HocusFocusParams {
                psf_type: PSFType::Moffat4,
                ..Default::default()
            };

            let detection_result =
                detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);

            // Convert to API response format
            let stars: Vec<StarInfo> = detection_result
                .stars
                .iter()
                .map(|star| {
                    let eccentricity = if let Some(psf) = &star.psf_model {
                        psf.eccentricity
                    } else {
                        0.0
                    };

                    StarInfo {
                        x: star.position.0,
                        y: star.position.1,
                        hfr: star.hfr,
                        fwhm: star.fwhm,
                        brightness: star.brightness,
                        eccentricity,
                    }
                })
                .collect();

            Ok::<(Vec<StarInfo>, usize, f64, f64), anyhow::Error>((
                stars,
                detection_result.stars.len(),
                detection_result.average_hfr,
                detection_result.average_fwhm,
            ))
        })
        .await
        .map_err(|e| AppError::InternalError(format!("Star detection task panicked: {}", e)))?
        .map_err(|e| AppError::InternalError(format!("Failed to detect stars: {}", e)))?;

    let response = StarDetectionResponse {
        detected_stars: detected_count,
        average_hfr,
        average_fwhm,
        stars,
    };

    // Save to cache
    let cached_data = serde_json::to_string(&response)
        .map_err(|_| AppError::InternalError("Failed to serialize response".to_string()))?;

    tokio::fs::write(&cache_path, cached_data)
        .await
        .map_err(|_| AppError::InternalError("Failed to write cache".to_string()))?;

    Ok(Json(ApiResponse::success(response)))
}

// Annotated (star-marked) image endpoint. Same async model as the preview:
// cache hit → 200 PNG; miss → enqueue on the interactive queue and 202.
#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_annotated_image(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(options): Query<PreviewOptions>,
) -> Result<Response, AppError> {
    let size = options.size.as_deref().unwrap_or("screen");
    let max_stars = options.max_stars.unwrap_or(1000) as usize;

    let (image, file_only, target_name) = resolve_image_meta(&ctx, image_id)?;
    let cache_key = annotated_cache_key(&image, &file_only, size, max_stars);
    let cache_path = artifact_cache_path(&ctx, "annotated", &cache_key)?;

    if cache_path.exists() {
        return serve_cached_png(&cache_path).await;
    }

    let fits_path = find_fits_file(&ctx, &image, &target_name, &file_only)?;
    state.enqueue_preview(crate::server::preview_queue::GenJob {
        fits_path,
        cache_path,
        kind: crate::server::preview_queue::GenKind::Annotated {
            max_stars,
            size: size.to_string(),
        },
    });
    Ok(generating_response())
}

/// One artifact whose readiness the frontend wants to know, sent in a batch so
/// a grid of generating images produces a single poll instead of one per image.
#[derive(Debug, Deserialize)]
pub struct GenStatusItem {
    pub image_id: i32,
    /// "preview" (default) or "annotated".
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub size: Option<String>,
    #[serde(default)]
    pub stretch: Option<bool>,
    #[serde(default)]
    pub midtone: Option<f64>,
    #[serde(default)]
    pub shadow: Option<f64>,
    #[serde(default)]
    pub max_stars: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct GenerationStatusRequest {
    pub requests: Vec<GenStatusItem>,
}

#[derive(Debug, Serialize)]
pub struct GenerationStatusBatch {
    /// Parallel to the request's `requests`.
    pub statuses: Vec<crate::server::preview_queue::GenerationStatus>,
}

/// POST /api/db/{db_id}/images/generation-status
///
/// Batch readiness poll for on-demand previews / annotated images. For each
/// item: cached → `ready`; generating → `generating`; failed → `error`;
/// unknown → enqueue (idempotent) and report `generating`. Coalesces a whole
/// grid's polling into one request instead of one-per-image.
pub async fn post_generation_status(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<GenerationStatusRequest>,
) -> Result<Json<ApiResponse<GenerationStatusBatch>>, AppError> {
    // Batch the DB work: one images lookup + one targets lookup for the whole
    // request, instead of two locked point queries per item. A DB error fails
    // the whole batch with a 5xx — which the frontend simply retries on the
    // next poll tick — so a transient lock never collapses into a permanent
    // per-tile "error"; only an id genuinely absent from the results is
    // terminal.
    let ids: Vec<i32> = req.requests.iter().map(|r| r.image_id).collect();
    let (images_by_id, target_names): (
        HashMap<i32, crate::models::AcquiredImage>,
        HashMap<i32, String>,
    ) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);
        let images = db.get_images_by_ids(&ids).map_err(AppError::db)?;
        let target_ids: Vec<i32> = images.iter().map(|i| i.target_id).collect();
        let targets = db.get_targets_by_ids(&target_ids).map_err(AppError::db)?;
        (
            images.into_iter().map(|i| (i.id, i)).collect(),
            targets.into_iter().map(|t| (t.id, t.name)).collect(),
        )
    };

    let statuses = req
        .requests
        .iter()
        .map(|item| status_for_item(&state, &ctx, item, &images_by_id, &target_names))
        .collect();
    Ok(Json(ApiResponse::success(GenerationStatusBatch {
        statuses,
    })))
}

/// Resolve one status item to a `GenerationStatus` from pre-fetched image /
/// target maps, enqueuing generation when the artifact is neither cached nor
/// already known to the queue. An id absent from `images_by_id` is a genuinely
/// unknown image (permanent `error`); transient DB failures are handled one
/// level up (the batch 5xx → frontend retry).
fn status_for_item(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    item: &GenStatusItem,
    images_by_id: &HashMap<i32, crate::models::AcquiredImage>,
    target_names: &HashMap<i32, String>,
) -> crate::server::preview_queue::GenerationStatus {
    use crate::server::preview_queue::{GenJob, GenKind, GenerationState, GenerationStatus};

    let err = |msg: &str| GenerationStatus {
        state: GenerationState::Error,
        error: Some(msg.to_string()),
    };

    let Some(image) = images_by_id.get(&item.image_id) else {
        return err("image not found");
    };
    let Some(target_name) = target_names.get(&image.target_id) else {
        return err("target not found");
    };
    let Some(file_only) = filename_from_metadata(&image.metadata) else {
        return err("no filename in metadata");
    };

    let size = item.size.clone().unwrap_or_else(|| "screen".to_string());
    let (cache_path, kind) = match item.kind.as_deref() {
        Some("annotated") => {
            let max_stars = item.max_stars.unwrap_or(1000) as usize;
            let key = annotated_cache_key(image, &file_only, &size, max_stars);
            match artifact_cache_path(ctx, "annotated", &key) {
                Ok(p) => (
                    p,
                    GenKind::Annotated {
                        max_stars,
                        size: size.clone(),
                    },
                ),
                Err(_) => return err("cache error"),
            }
        }
        _ => {
            let stretch = item.stretch.unwrap_or(true);
            let midtone = item.midtone.unwrap_or(0.2);
            let shadow = item.shadow.unwrap_or(-2.8);
            let key = preview_cache_key(image, &file_only, &size, stretch, midtone, shadow);
            match artifact_cache_path(ctx, "previews", &key) {
                Ok(p) => (
                    p,
                    GenKind::Preview {
                        midtone,
                        shadow,
                        max_dimensions: crate::server::preview_queue::max_dimensions_for_size(
                            &size,
                        ),
                    },
                ),
                Err(_) => return err("cache error"),
            }
        }
    };

    if let Some(status) = state.preview_queue.status(&cache_path) {
        return status;
    }

    // Neither cached, in-flight, nor errored: ensure it's enqueued to generate.
    match find_fits_file(ctx, image, target_name, &file_only) {
        Ok(fits_path) => {
            state.enqueue_preview(GenJob {
                fits_path,
                cache_path,
                kind,
            });
            GenerationStatus {
                state: GenerationState::Generating,
                error: None,
            }
        }
        Err(_) => err("source file not found"),
    }
}

// PSF multi image parameters
#[derive(Deserialize)]
pub struct PsfMultiOptions {
    pub num_stars: Option<usize>,
    pub psf_type: Option<String>,
    pub sort_by: Option<String>,
    pub grid_cols: Option<usize>,
    pub selection: Option<String>,
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_psf_visualization(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
    Query(options): Query<PsfMultiOptions>,
) -> Result<impl IntoResponse, AppError> {
    use crate::commands::visualize_psf_multi_common::create_psf_multi_image;
    use crate::image_analysis::FitsImage;
    use crate::psf_fitting::PSFType;
    use crate::server::cache::CacheManager;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ColorType, ImageEncoder};

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(AppError::db)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    // Parse parameters
    let num_stars = options.num_stars.unwrap_or(9);
    let psf_type_str = options.psf_type.as_deref().unwrap_or("moffat").to_string();
    let sort_by = options.sort_by.as_deref().unwrap_or("r2").to_string();
    let selection = options.selection.as_deref().unwrap_or("top-n").to_string();
    let grid_cols = options.grid_cols;

    let psf_type: PSFType = psf_type_str.parse().unwrap_or(PSFType::Moffat4);

    // Create comprehensive cache key for PSF multi image
    let cache_key = format!(
        "psf_multi_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        num_stars,
        psf_type_str,
        sort_by,
        selection,
        grid_cols.unwrap_or(0)
    );
    let cache_manager = CacheManager::new(PathBuf::from(&ctx.cache_dir));
    cache_manager
        .ensure_category_dir("psf_multi")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("psf_multi", &cache_key, "png");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        // Serve from cache
        let mut file = File::open(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .await
            .map_err(|_| AppError::InternalError("Failed to read file".to_string()))?;

        return Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/png"),
                (CACHE_CONTROL, "max-age=86400"), // TODO: Use configurable cache expiry
            ],
            buffer,
        ));
    }

    // Find FITS file path first (this is fast)
    let fits_path = find_fits_file(&ctx, &image, &target_name, &file_only)?;

    // Move expensive operations to spawn_blocking
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let cache_path_clone = cache_path.clone();
    tokio::task::spawn_blocking(move || {
        // Load FITS file
        let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))
            .map_err(|e| anyhow::anyhow!("Failed to load FITS: {}", e))?;

        // Create PSF multi visualization using the common function
        let rgba_image =
            create_psf_multi_image(&fits, num_stars, psf_type, &sort_by, grid_cols, &selection)
                .map_err(|e| anyhow::anyhow!("Failed to create PSF visualization: {}", e))?;

        // Save to cache
        let cache_file = std::fs::File::create(&cache_path_clone)
            .map_err(|e| anyhow::anyhow!("Failed to create cache file: {}", e))?;
        let writer = std::io::BufWriter::new(cache_file);
        let encoder =
            PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

        encoder
            .write_image(
                &rgba_image,
                rgba_image.width(),
                rgba_image.height(),
                ColorType::Rgba8.into(),
            )
            .map_err(|e| anyhow::anyhow!("Failed to encode PNG: {}", e))?;

        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|e| AppError::InternalError(format!("PSF visualization task panicked: {}", e)))?
    .map_err(|e| AppError::InternalError(format!("Failed to generate PSF visualization: {}", e)))?;

    // Read the cached file
    let png_buffer = tokio::fs::read(&cache_path)
        .await
        .map_err(|_| AppError::InternalError("Failed to read generated PNG".to_string()))?;

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
        ],
        png_buffer,
    ))
}

// Overview API endpoints
pub async fn get_projects_overview(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<ProjectOverviewResponse>>>, AppError> {
    tracing::debug!("📋 Getting projects overview");

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    // Get all projects with images and profile info
    let projects = db
        .get_projects_with_images_and_profile_info()
        .map_err(AppError::db)?;

    // Get profile count to determine display format
    let profile_count = db.get_profile_count().map_err(AppError::db)?;

    // Get file existence map
    let file_existence_map: std::collections::HashMap<i32, bool> = {
        let cache = ctx.file_check_cache.read().unwrap();
        cache.projects_with_files.clone()
    };
    let mut recent_images_by_project: HashMap<i32, Vec<crate::models::RecentImageSummary>> =
        HashMap::new();
    for image in db.get_recent_images_by_project(3).map_err(AppError::db)? {
        recent_images_by_project
            .entry(image.project_id)
            .or_default()
            .push(image);
    }

    let mut response = Vec::new();
    let show_profile = profile_count > 1;

    for project_with_profile in projects {
        let project = &project_with_profile.project;
        // Get detailed stats for this project
        let stats = db
            .get_project_overview_stats(project.id)
            .map_err(AppError::db)?;

        // Get desired values for this project
        let desired_stats =
            db.get_project_desired_stats(project.id)
                .unwrap_or(ProjectDesiredStats {
                    total_desired: 0,
                    total_acquired: 0,
                    total_accepted: 0,
                    rejected_count: 0,
                    filters_used: vec![],
                });

        let target_count = db
            .get_target_count_for_project(project.id)
            .map_err(AppError::db)?;

        // Get basic file statistics (simplified for performance)
        let files_found = if file_existence_map
            .get(&project.id)
            .copied()
            .unwrap_or(false)
        {
            stats.total_images // Optimistic assumption: if we think project has files, assume all files exist
        } else {
            0
        };
        let files_missing = stats.total_images - files_found;

        let span_days = match (stats.earliest_date, stats.latest_date) {
            (Some(start), Some(end)) => {
                let days = (end - start) / 86400; // seconds to days
                Some(days as i32)
            }
            _ => None,
        };

        let display_name = if show_profile {
            format!("{} → {}", project_with_profile.profile_name, project.name)
        } else {
            project.name.clone()
        };

        response.push(ProjectOverviewResponse {
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
            target_count,
            total_images: stats.total_images,
            accepted_images: stats.accepted_images,
            rejected_images: stats.rejected_images,
            pending_images: stats.pending_images,
            total_desired: desired_stats.total_desired,
            files_found,
            files_missing,
            date_range: DateRange {
                earliest: stats.earliest_date,
                latest: stats.latest_date,
                span_days,
            },
            filters_used: stats.filters_used,
            recent_images: recent_images_by_project
                .remove(&project.id)
                .unwrap_or_default(),
        });
    }

    Ok(Json(ApiResponse::success(response)))
}

pub async fn get_targets_overview(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<TargetOverviewResponse>>>, AppError> {
    tracing::debug!("🎯 Getting targets overview");

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    // Get all targets with project info and stats including desired values
    let targets_data = db
        .get_all_targets_with_desired_stats()
        .map_err(AppError::db)?;

    // Get file existence map
    let file_existence_map: std::collections::HashMap<i32, bool> = {
        let cache = ctx.file_check_cache.read().unwrap();
        cache.targets_with_files.clone()
    };

    let mut response = Vec::new();

    for target_data in targets_data {
        // Get date range and filters for this target
        let (earliest, latest, filters) = {
            let mut stmt = conn.prepare(
                "SELECT MIN(acquireddate), MAX(acquireddate) FROM acquiredimage WHERE targetId = ?",
            ).map_err(AppError::db)?;

            let (earliest, latest): (Option<i64>, Option<i64>) = stmt
                .query_row([target_data.target.id], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .map_err(AppError::db)?;

            let mut filter_stmt = conn.prepare(
                "SELECT DISTINCT filtername FROM acquiredimage WHERE targetId = ? AND filtername IS NOT NULL ORDER BY filtername",
            ).map_err(AppError::db)?;

            let filters: Vec<String> = filter_stmt
                .query_map([target_data.target.id], |row| row.get(0))
                .map_err(AppError::db)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(AppError::db)?;

            (earliest, latest, filters)
        };

        let span_days = match (earliest, latest) {
            (Some(start), Some(end)) => {
                let days = (end - start) / 86400;
                Some(days as i32)
            }
            _ => None,
        };

        // Get basic file statistics (simplified for performance)
        let files_found = if file_existence_map
            .get(&target_data.target.id)
            .copied()
            .unwrap_or(false)
        {
            target_data.total_images // Optimistic assumption: if we think target has files, assume all files exist
        } else {
            0
        };
        let files_missing = target_data.total_images - files_found;

        response.push(TargetOverviewResponse {
            id: target_data.target.id,
            name: target_data.target.name.clone(),
            ra: target_data.target.ra,
            dec: target_data.target.dec,
            active: target_data.target.active,
            project_id: target_data.target.project_id,
            project_name: target_data.project_name,
            image_count: target_data.total_images,
            accepted_count: target_data.accepted_images,
            rejected_count: target_data.rejected_images,
            pending_count: target_data.pending_images,
            total_desired: target_data.total_desired,
            files_found,
            files_missing,
            has_files: file_existence_map
                .get(&target_data.target.id)
                .copied()
                .unwrap_or(false),
            date_range: DateRange {
                earliest,
                latest,
                span_days,
            },
            filters_used: filters,
            coordinates_display: format_coordinates(target_data.target.ra, target_data.target.dec),
        });
    }

    Ok(Json(ApiResponse::success(response)))
}

pub async fn get_overall_stats(
    ctx: DbContext,
) -> Result<Json<ApiResponse<OverallStatsResponse>>, AppError> {
    tracing::debug!("📊 Getting overall statistics");

    let conn = ctx.db();
    let conn = conn.lock().map_err(AppError::db)?;
    let db = Database::new(&conn);

    let stats = db.get_overall_statistics().map_err(AppError::db)?;

    // Get overall desired statistics
    let desired_stats = db
        .get_overall_desired_statistics()
        .unwrap_or(OverallDesiredStats {
            total_desired: 0,
            total_acquired: 0,
            total_accepted: 0,
        });

    let span_days = match (stats.earliest_date, stats.latest_date) {
        (Some(start), Some(end)) => {
            let days = (end - start) / 86400;
            Some(days as i32)
        }
        _ => None,
    };

    // Calculate overall file statistics (simplified for performance)
    let total_files_found = if stats.active_projects > 0 {
        stats.total_images
    } else {
        0
    }; // Optimistic: assume all files exist for active projects
    let total_files_missing = stats.total_images - total_files_found;

    // For now, we'll return empty recent activity - this could be enhanced later
    let recent_activity = Vec::new();

    let response = OverallStatsResponse {
        total_projects: stats.total_projects,
        active_projects: stats.active_projects,
        total_targets: stats.total_targets,
        active_targets: stats.active_targets,
        total_images: stats.total_images,
        accepted_images: stats.accepted_images,
        rejected_images: stats.rejected_images,
        pending_images: stats.pending_images,
        total_desired: desired_stats.total_desired,
        files_found: total_files_found,
        files_missing: total_files_missing,
        unique_filters: stats.unique_filters,
        date_range: DateRange {
            earliest: stats.earliest_date,
            latest: stats.latest_date,
            span_days,
        },
        recent_activity,
    };

    Ok(Json(ApiResponse::success(response)))
}

// Sequence analysis handlers

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn analyze_sequence(
    ctx: DbContext,
    Query(params): Query<crate::server::api::SequenceAnalysisQuery>,
) -> Result<Json<ApiResponse<crate::server::api::SequenceAnalysisResponse>>, AppError> {
    use crate::sequence_analysis::{
        extract_metrics_from_metadata, QualityWeights, SequenceAnalyzer, SequenceAnalyzerConfig,
    };

    let target_id = params.target_id;
    let filter_name = params.filter_name.clone();
    let session_gap = params.session_gap_minutes;
    let weight_star_count = params.weight_star_count;
    let weight_hfr = params.weight_hfr;
    let weight_eccentricity = params.weight_eccentricity;
    let weight_snr = params.weight_snr;
    let weight_background = params.weight_background;
    let weight_spatial = params.weight_spatial;
    let weight_pointing = params.weight_pointing;

    // Fetch images from database
    let (images_data, target_name, expected_by_image) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        // Get target name
        let targets = db.get_targets_by_ids(&[target_id]).map_err(AppError::db)?;
        let target = targets
            .into_iter()
            .next()
            .ok_or_else(|| AppError::BadRequest(format!("Target {} not found", target_id)))?;
        let target_name = target.name.clone();

        // Query images for this target
        let all_images = db
            .query_images_scoped(None, None, Some(target_id), None, 0)
            .map_err(AppError::db)?;

        let filtered: Vec<_> = all_images
            .into_iter()
            .filter(|(img, _, _)| {
                img.target_id == target_id
                    && filter_name.as_ref().is_none_or(|f| img.filter_name == *f)
            })
            .collect();
        let mut resolver =
            crate::acquisition_context::FramingResolver::new(&conn).map_err(AppError::db)?;
        let expected_by_image = filtered
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_for_grading(&conn, image)
                    .map(|expected| (image.id, expected))
            })
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(AppError::db)?;

        (filtered, target_name, expected_by_image)
    };

    if images_data.is_empty() {
        return Ok(Json(ApiResponse::success(
            crate::server::api::SequenceAnalysisResponse { sequences: vec![] },
        )));
    }

    // Extract metrics and group by filter. A prior quality scan supplies fresh
    // star/HFR measurements plus the spatial fields N.I.N.A. does not store.
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let spatial_store = ctx.spatial_metrics.clone();
    let astrometry_cache_dir = ctx.cache_dir_path.clone();
    let astrometry_evidence = ctx.astrometry_evidence.clone();
    let target_name_clone = target_name.clone();
    let result = tokio::task::spawn_blocking(move || {
        let mut config = SequenceAnalyzerConfig::default();
        if let Some(gap) = session_gap {
            config.session_gap_minutes = gap;
        }
        // Apply weight overrides from query params if any are provided
        if weight_star_count.is_some()
            || weight_hfr.is_some()
            || weight_eccentricity.is_some()
            || weight_snr.is_some()
            || weight_background.is_some()
            || weight_spatial.is_some()
            || weight_pointing.is_some()
        {
            config.quality_weights = QualityWeights {
                star_count: weight_star_count.unwrap_or(config.quality_weights.star_count),
                hfr: weight_hfr.unwrap_or(config.quality_weights.hfr),
                eccentricity: weight_eccentricity.unwrap_or(config.quality_weights.eccentricity),
                snr: weight_snr.unwrap_or(config.quality_weights.snr),
                background: weight_background.unwrap_or(config.quality_weights.background),
                spatial: weight_spatial.unwrap_or(config.quality_weights.spatial),
                transparency: config.quality_weights.transparency,
                pointing: weight_pointing.unwrap_or(config.quality_weights.pointing),
            };
        }

        let session_gap_minutes = config.session_gap_minutes;
        let analyzer = SequenceAnalyzer::new(config);

        // Group by filter_name and analyze each group
        let mut by_filter: std::collections::HashMap<String, Vec<_>> =
            std::collections::HashMap::new();
        let mut entries_by_filter: std::collections::HashMap<String, Vec<_>> =
            std::collections::HashMap::new();
        for (img, _proj, _target) in &images_data {
            let mut metrics =
                extract_metrics_from_metadata(img.id, &img.metadata, img.acquired_date);
            merge_spatial_metrics(&mut metrics, &spatial_store, &img.metadata);
            merge_astrometry_metrics(
                &mut metrics,
                &astrometry_cache_dir,
                &img.metadata,
                &astrometry_evidence,
                expected_by_image.get(&img.id).copied().flatten(),
            );
            entries_by_filter
                .entry(img.filter_name.clone())
                .or_default()
                .push(stored_entry_for(&spatial_store, img.id, &img.metadata));
            by_filter
                .entry(img.filter_name.clone())
                .or_default()
                .push(metrics);
        }

        let mut all_sequences = Vec::new();
        for (filter, mut metrics) in by_filter {
            if let Some(entries) = entries_by_filter.get(&filter) {
                merge_photometric_signals(&mut metrics, entries, session_gap_minutes);
            }
            let scored = analyzer.analyze(&metrics, target_id, &target_name_clone, &filter);
            all_sequences.extend(scored);
        }

        all_sequences
    })
    .await
    .map_err(|e| AppError::InternalError(format!("Analysis task failed: {}", e)))?;

    let mut sequences: Vec<_> = result
        .into_iter()
        .map(|seq| crate::server::api::ScoredSequenceResponse {
            target_id: seq.target_id,
            target_name: seq.target_name,
            filter_name: seq.filter_name,
            session_start: seq.session_start,
            session_end: seq.session_end,
            image_count: seq.image_count,
            reference_values: seq.reference_values,
            images: seq.images,
            summary: seq.summary,
        })
        .collect();
    sequences.sort_by(|a, b| {
        b.session_start
            .cmp(&a.session_start)
            .then_with(|| a.filter_name.cmp(&b.filter_name))
    });

    Ok(Json(ApiResponse::success(
        crate::server::api::SequenceAnalysisResponse { sequences },
    )))
}

#[axum::debug_handler(state = Arc<AppState>)]
pub async fn get_image_quality(
    ctx: DbContext,
    Path((_db_id, image_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<crate::server::api::ImageQualityContextResponse>>, AppError> {
    use crate::sequence_analysis::{
        extract_metrics_from_metadata, SequenceAnalyzer, SequenceAnalyzerConfig,
    };

    // Get the target image and its context from database
    let (target_image, all_filter_images, target_name, expected_by_image) = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let images = db.get_images_by_ids(&[image_id]).map_err(AppError::db)?;
        let target_image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[target_image.target_id])
            .map_err(AppError::db)?;
        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        // Get all images for the same target + filter
        let all_images = db
            .query_images_scoped(None, None, Some(target_image.target_id), None, 0)
            .map_err(AppError::db)?;

        let filter_images: Vec<_> = all_images
            .into_iter()
            .filter(|(img, _, _)| {
                img.target_id == target_image.target_id
                    && img.filter_name == target_image.filter_name
            })
            .collect();
        let mut resolver =
            crate::acquisition_context::FramingResolver::new(&conn).map_err(AppError::db)?;
        let expected_by_image = filter_images
            .iter()
            .map(|(image, _, _)| {
                resolver
                    .expected_for_grading(&conn, image)
                    .map(|expected| (image.id, expected))
            })
            .collect::<Result<std::collections::HashMap<_, _>, _>>()
            .map_err(AppError::db)?;

        (target_image, filter_images, target_name, expected_by_image)
    };

    if all_filter_images.is_empty() {
        return Ok(Json(ApiResponse::success(
            crate::server::api::ImageQualityContextResponse {
                image_id,
                quality: None,
                sequence_target_id: None,
                sequence_filter_name: None,
                sequence_image_count: None,
                reference_values: None,
            },
        )));
    }

    let filter_name = target_image.filter_name.clone();
    let filter_name_for_task = filter_name.clone();
    let seq_target_id = target_image.target_id;
    let target_name_clone = target_name.clone();
    crate::server::spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let spatial_store = ctx.spatial_metrics.clone();
    let astrometry_cache_dir = ctx.cache_dir_path.clone();
    let astrometry_evidence = ctx.astrometry_evidence.clone();

    let result = tokio::task::spawn_blocking(move || {
        let config = SequenceAnalyzerConfig::default();
        let session_gap_minutes = config.session_gap_minutes;
        let analyzer = SequenceAnalyzer::new(config);

        let mut metrics: Vec<_> = Vec::with_capacity(all_filter_images.len());
        let mut entries = Vec::with_capacity(all_filter_images.len());
        for (img, _, _) in &all_filter_images {
            let mut m = extract_metrics_from_metadata(img.id, &img.metadata, img.acquired_date);
            merge_spatial_metrics(&mut m, &spatial_store, &img.metadata);
            merge_astrometry_metrics(
                &mut m,
                &astrometry_cache_dir,
                &img.metadata,
                &astrometry_evidence,
                expected_by_image.get(&img.id).copied().flatten(),
            );
            entries.push(stored_entry_for(&spatial_store, img.id, &img.metadata));
            metrics.push(m);
        }
        merge_photometric_signals(&mut metrics, &entries, session_gap_minutes);

        analyzer.analyze(
            &metrics,
            seq_target_id,
            &target_name_clone,
            &filter_name_for_task,
        )
    })
    .await
    .map_err(|e| AppError::InternalError(format!("Analysis task failed: {}", e)))?;

    // Find our image in the results
    for seq in &result {
        if let Some(quality) = seq.images.iter().find(|r| r.image_id == image_id) {
            return Ok(Json(ApiResponse::success(
                crate::server::api::ImageQualityContextResponse {
                    image_id,
                    quality: Some(quality.clone()),
                    sequence_target_id: Some(seq.target_id),
                    sequence_filter_name: Some(seq.filter_name.clone()),
                    sequence_image_count: Some(seq.image_count),
                    reference_values: Some(seq.reference_values.clone()),
                },
            )));
        }
    }

    // Image was not in any scored sequence (too short, etc.)
    Ok(Json(ApiResponse::success(
        crate::server::api::ImageQualityContextResponse {
            image_id,
            quality: None,
            sequence_target_id: Some(seq_target_id),
            sequence_filter_name: Some(filter_name),
            sequence_image_count: None,
            reference_values: None,
        },
    )))
}

// ---------------- Spatial (occlusion) metrics scan ----------------

/// Extract the FITS basename from an acquiredimage metadata JSON blob.
pub(crate) fn filename_from_metadata(metadata_json: &str) -> Option<String> {
    let metadata: serde_json::Value = serde_json::from_str(metadata_json).ok()?;
    let filename = metadata["FileName"].as_str()?;
    filename
        .split(&['\\', '/'][..])
        .next_back()
        .map(|s| s.to_string())
}

/// Fetch the filename-validated stored scan entry for an image, if any.
pub(crate) fn stored_entry_for(
    store: &crate::server::spatial_scan::SharedSpatialStore,
    image_id: i32,
    metadata_json: &str,
) -> Option<crate::server::spatial_scan::StoredSpatialMetrics> {
    let file_only = filename_from_metadata(metadata_json)?;
    crate::server::spatial_scan::valid_quality_entry(store, image_id, &file_only)
}

/// Run the cross-frame photometric pass (transparency, localized extinction,
/// per-cell temporal baselines) over one filter group and merge the signals
/// into its `ImageMetrics`. `entries` parallels `metrics` index-for-index;
/// frames without a stored scan entry contribute empty inputs and receive no
/// signals. Frames are bucketed by exposure (flux ratios are only meaningful
/// within one exposure length) and split into sessions before the pass.
pub(crate) fn merge_photometric_signals(
    metrics: &mut [crate::sequence_analysis::ImageMetrics],
    entries: &[Option<crate::server::spatial_scan::StoredSpatialMetrics>],
    session_gap_minutes: u64,
) {
    use crate::photometry::{
        sequence_screening_signals, split_sessions, FrameInputs, PhotometryConfig,
    };

    if metrics.len() != entries.len() {
        return;
    }

    // Time order (the analyzer applies the same stable sort later).
    let mut order: Vec<usize> = (0..metrics.len()).collect();
    order.sort_by_key(|&i| (metrics[i].timestamp.unwrap_or(0), metrics[i].image_id));

    // Bucket by exposure seconds.
    let mut buckets: std::collections::BTreeMap<i64, Vec<usize>> =
        std::collections::BTreeMap::new();
    for &i in &order {
        let key = entries[i]
            .as_ref()
            .and_then(|e| e.exposure_s)
            .map(|e| e.round() as i64)
            .unwrap_or(-1);
        buckets.entry(key).or_default().push(i);
    }

    let phot_config = PhotometryConfig::default();
    let gap_seconds = (session_gap_minutes * 60) as i64;
    for indices in buckets.values() {
        let timestamps: Vec<Option<i64>> = indices.iter().map(|&i| metrics[i].timestamp).collect();
        for session in split_sessions(&timestamps, gap_seconds) {
            let session_idx: Vec<usize> = session.iter().map(|&s| indices[s]).collect();
            let inputs: Vec<FrameInputs> = session_idx
                .iter()
                .map(|&i| match &entries[i] {
                    Some(e) => FrameInputs {
                        catalog: e.catalog.clone(),
                        star_cell_counts: e.star_cell_counts.clone(),
                        bg_cell_medians: e.bg_cell_medians.clone(),
                    },
                    None => FrameInputs::default(),
                })
                .collect();
            let dims = session_idx.iter().find_map(|&i| {
                entries[i].as_ref().and_then(|e| {
                    (e.width > 0 && e.grid_cols > 0).then_some((
                        e.width,
                        e.height,
                        e.grid_cols,
                        e.grid_rows,
                    ))
                })
            });
            let Some((width, height, grid_cols, grid_rows)) = dims else {
                continue;
            };
            let signals = sequence_screening_signals(
                &inputs,
                width,
                height,
                (grid_cols, grid_rows),
                &phot_config,
            );
            for (&i, sig) in session_idx.iter().zip(signals) {
                let m = &mut metrics[i];
                m.transparency = sig.transparency;
                m.extinction_cell_fraction = sig.extinction_cell_fraction;
                m.star_cell_drop_fraction = sig.star_cell_drop_fraction;
                m.bg_cell_rise_fraction = sig.bg_cell_rise_fraction;
                m.bg_cell_fall_fraction = sig.bg_cell_fall_fraction;
            }
        }
    }
}

/// Merge fresh detector and spatial results from the per-DB quality cache.
/// A quality scan is the source of truth for star count and HFR once present;
/// the spatial fields fill values that N.I.N.A. does not store.
pub(crate) fn merge_spatial_metrics(
    metrics: &mut crate::sequence_analysis::ImageMetrics,
    store: &crate::server::spatial_scan::SharedSpatialStore,
    metadata_json: &str,
) {
    let Some(file_only) = filename_from_metadata(metadata_json) else {
        return;
    };
    if let Some(entry) =
        crate::server::spatial_scan::valid_entry(store, metrics.image_id, &file_only)
    {
        if entry.detector == crate::server::spatial_scan::QUALITY_DETECTOR
            && entry.detector_version == crate::server::spatial_scan::QUALITY_DETECTOR_VERSION
        {
            metrics.star_count = Some(entry.star_count as f64);
            metrics.hfr = (entry.avg_hfr > 0.0).then_some(entry.avg_hfr);
        }
        if metrics.dead_cell_fraction.is_none() {
            metrics.dead_cell_fraction = entry.dead_cell_fraction;
        }
        if metrics.bg_cell_spread.is_none() {
            metrics.bg_cell_spread = Some(entry.bg_cell_spread);
        }
        if metrics.bg_glow_max.is_none() && entry.bg_glow_max > 0.0 {
            metrics.bg_glow_max = Some(entry.bg_glow_max);
        }
    }
}

pub(crate) fn merge_astrometry_metrics(
    metrics: &mut crate::sequence_analysis::ImageMetrics,
    cache_dir: &std::path::Path,
    metadata_json: &str,
    evidence: &crate::astrometry::AstrometryEvidenceCache,
    expected_target: Option<(f64, f64)>,
) {
    let Some(file_only) = filename_from_metadata(metadata_json) else {
        return;
    };
    let Some(analysis) = evidence.evidence_for_source(cache_dir, metrics.image_id, expected_target)
    else {
        return;
    };
    let cached_file = std::path::Path::new(&analysis.source_fingerprint.canonical_path)
        .file_name()
        .and_then(|name| name.to_str());
    if cached_file != Some(file_only.as_str()) {
        return;
    }
    metrics.astrometry = crate::sequence_analysis::astrometry_metrics_from_analysis(&analysis);
    metrics.satellite =
        crate::satellites::persisted_analysis(cache_dir, metrics.image_id, &analysis)
            .as_ref()
            .map(crate::sequence_analysis::SatelliteFrameMetrics::from);
}

/// POST /api/db/{db_id}/analysis/spatial-scan
///
/// Start a background scan computing spatial occlusion metrics from the FITS
/// files of a target's images. Singleton per database: if a scan is already
/// running (or nothing needs computing) this returns `started: false` with
/// the current progress. Poll GET on the same path for progress.
pub async fn start_spatial_scan(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<SpatialScanRequest>,
) -> Result<Json<ApiResponse<SpatialScanStatusResponse>>, AppError> {
    start_spatial_scan_with_priority(
        State(state),
        ctx,
        Json(req),
        crate::concurrency::Priority::Interactive,
        true,
    )
    .await
}

async fn start_spatial_scan_with_priority(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(req): Json<SpatialScanRequest>,
    priority: crate::concurrency::Priority,
    include_satellites: bool,
) -> Result<Json<ApiResponse<SpatialScanStatusResponse>>, AppError> {
    use crate::server::spatial_scan as scan;

    scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);

    // This is an explicit user-triggered background action, so it may seed the
    // durable orbital cache for an exposure. Read-only sequence queries and
    // CLI regrading remain cache-only.
    let satellite_context = Arc::clone(&state.satellites);

    // Collect this target's images together with schema-adaptive intended
    // framing. Unsupported coordinate epochs are deliberately omitted from
    // absolute grading, though the solver may still use FITS hints.
    let candidates = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        let db = Database::new(&conn);

        let targets = db
            .get_targets_by_ids(&[req.target_id])
            .map_err(AppError::db)?;
        let target = targets
            .into_iter()
            .next()
            .ok_or_else(|| AppError::BadRequest(format!("Target {} not found", req.target_id)))?;
        let target_name = target.name.clone();

        let all_images = db
            .query_images_scoped(None, None, Some(req.target_id), None, 0)
            .map_err(AppError::db)?;

        let mut resolver =
            crate::acquisition_context::FramingResolver::new(&conn).map_err(AppError::db)?;
        let mut candidates = Vec::new();
        for (img, _, _) in all_images.into_iter().filter(|(img, _, _)| {
            img.target_id == req.target_id
                && req
                    .filter_name
                    .as_ref()
                    .is_none_or(|f| img.filter_name == *f)
        }) {
            let expected = resolver
                .expected_for_grading(&conn, &img)
                .map_err(AppError::db)?;
            candidates.push((img, target_name.clone(), expected));
        }
        candidates
    };

    // Keep the union of spatial, astrometry, and satellite work. One side may
    // already be cached while another still needs computation.
    let mut work = Vec::new();
    let mut skipped_cached = 0usize;
    let force_spatial = req.force || req.force_spatial;
    let force_astrometry = req.force || req.force_astrometry;
    let force_satellites = req.force || req.force_satellites;
    for (img, target_name, expected) in candidates {
        let Some(file_only) = filename_from_metadata(&img.metadata) else {
            continue;
        };
        let spatial_cached = !force_spatial
            && scan::valid_quality_entry(&ctx.spatial_metrics, img.id, &file_only).is_some();
        if spatial_cached {
            skipped_cached += 1;
        }
        let cached_astrometry = (!force_astrometry)
            .then(|| {
                state.astrometry.validated_persisted_pixel_analysis(
                    &ctx.cache_dir_path,
                    img.id,
                    expected,
                )
            })
            .flatten()
            .filter(|analysis| {
                std::path::Path::new(&analysis.source_fingerprint.canonical_path)
                    .file_name()
                    .and_then(|name| name.to_str())
                    == Some(file_only.as_str())
            });
        let astrometry_cached = cached_astrometry.is_some();
        let satellite_cached = !include_satellites
            || (!force_satellites
                && cached_astrometry.as_ref().is_some_and(|analysis| {
                    crate::satellites::persisted_analysis(&ctx.cache_dir_path, img.id, analysis)
                        .is_some()
                }));
        if !spatial_cached || !astrometry_cached || !satellite_cached {
            work.push((
                img,
                target_name,
                file_only,
                expected,
                !spatial_cached,
                !astrometry_cached,
                !satellite_cached,
            ));
        }
    }

    if work.is_empty() {
        let (progress, cached_count) = scan::progress_snapshot(&ctx.spatial_metrics);
        return Ok(Json(ApiResponse::success(SpatialScanStatusResponse {
            started: false,
            progress,
            cached_count,
        })));
    }

    if !scan::try_begin_scan(
        &ctx.spatial_metrics,
        req.target_id,
        req.filter_name.clone(),
        work.iter().filter(|item| item.4).count(),
        skipped_cached,
    ) {
        let (progress, cached_count) = scan::progress_snapshot(&ctx.spatial_metrics);
        return Ok(Json(ApiResponse::success(SpatialScanStatusResponse {
            started: false,
            progress,
            cached_count,
        })));
    }

    tracing::info!(
        "📐 Quality scan started for db={} target={} ({} images, {} spatial cached)",
        ctx.id,
        req.target_id,
        work.len(),
        skipped_cached
    );

    let ctx_arc = ctx.0.clone();
    let target_id = req.target_id;
    let worker_policy = state.worker_policy();
    let astrometry = Arc::clone(&state.astrometry);
    let runtime_handle = tokio::runtime::Handle::current();
    // Mark this as an interactive job for its whole lifetime so background
    // pre-generation yields cores + memory to it. Moved into the blocking task
    // and dropped when the scan returns (or panics).
    let interactive_guard = (priority == crate::concurrency::Priority::Interactive)
        .then(|| state.begin_interactive_job());
    let scheduling_state = Arc::clone(&state);
    tokio::task::spawn_blocking(move || {
        let _interactive_guard = interactive_guard;
        // Any panic below would be silently swallowed by tokio (the join
        // handle is dropped) and leave the singleton wedged at running=true;
        // catch it and always finalize.
        let scan_body = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // Resolve FITS paths first; unresolvable files count as errors.
            let mut items = Vec::with_capacity(work.len());
            for (
                img,
                target_name,
                file_only,
                expected,
                need_spatial,
                need_astrometry,
                need_satellite,
            ) in &work
            {
                match find_fits_file(&ctx_arc, img, target_name, file_only) {
                    Ok(path) => items.push((
                        crate::server::spatial_scan::ScanWorkItem {
                            image_id: img.id,
                            filename: file_only.clone(),
                            fits_path: path,
                        },
                        *expected,
                        *need_spatial,
                        *need_astrometry,
                        *need_satellite,
                    )),
                    Err(_) => {
                        let mut s = ctx_arc.spatial_metrics.write().unwrap();
                        // The stage total counts only spatial work; advancing
                        // it for an astrometry-only item would overshoot it.
                        if *need_spatial {
                            s.progress.processed += 1;
                        }
                        s.progress.errors += 1;
                        s.progress.last_error = Some(format!("{}: FITS file not found", file_only));
                    }
                }
            }
            let spatial_items = items
                .iter()
                .filter(|item| item.2)
                .map(|item| item.0.clone())
                .collect::<Vec<_>>();
            // Size the worker pool to the machine (configured core ratio) and
            // the sensor (peak memory of one in-flight frame, probed from the
            // first resolvable file). Leaves headroom for request serving.
            let frame_pixels = spatial_items
                .first()
                .and_then(|it| crate::concurrency::probe_frame_pixels(&it.fits_path));
            let budget =
                crate::concurrency::plan_workers(None, &worker_policy, priority, frame_pixels);
            let wait_for_turn = || {
                while priority == crate::concurrency::Priority::Background
                    && scheduling_state.interactive_job_active()
                {
                    std::thread::sleep(std::time::Duration::from_millis(250));
                }
            };
            if !spatial_items.is_empty() {
                tracing::info!(
                    "📐 Spatial scan concurrency: {} worker(s) — {}",
                    budget.workers,
                    budget.rationale
                );
                crate::server::spatial_scan::run_scan(
                    &ctx_arc.spatial_metrics,
                    &ctx_arc.cache_dir_path,
                    &spatial_items,
                    budget.workers,
                    &wait_for_turn,
                );
            }

            let astrometry_items = items
                .iter()
                .filter(|item| item.3 || item.4)
                .collect::<Vec<_>>();
            if !astrometry_items.is_empty() {
                crate::server::spatial_scan::begin_astrometry_stage(
                    &ctx_arc.spatial_metrics,
                    astrometry_items.len(),
                );
                for (item, expected, _, need_astrometry, need_satellite) in astrometry_items {
                    wait_for_turn();
                    crate::server::spatial_scan::begin_astrometry_item(
                        &ctx_arc.spatial_metrics,
                        &item.filename,
                    );
                    // Acquire the per-database solve mutex per image, not for
                    // the whole stage: a user-triggered on-demand solve must be
                    // able to interleave with a long-running scan.
                    let outcome = if *need_astrometry {
                        let solve_guard = ctx_arc.astrometry_solve_mutex.blocking_lock();
                        let outcome = astrometry.solve_image_for_quality(
                            item.image_id,
                            &item.fits_path,
                            *expected,
                        );
                        drop(solve_guard);
                        outcome
                    } else {
                        astrometry
                            .validated_persisted_pixel_analysis(
                                &ctx_arc.cache_dir_path,
                                item.image_id,
                                *expected,
                            )
                            .ok_or_else(|| "cached plate solution became unavailable".to_string())
                    };
                    match outcome {
                        Ok(analysis) => {
                            let attempt = analysis.solve_attempt.as_ref();
                            let solved = attempt.is_some_and(|attempt| {
                                attempt.outcome
                                    == crate::astrometry::AstrometryAttemptOutcome::Solved
                            });
                            let quality_failure = attempt
                                .is_some_and(|attempt| attempt.image_quality_evidence && !solved);
                            let operational_error =
                                if attempt.is_some_and(|attempt| attempt.cacheable) {
                                    crate::astrometry::persist_pixel_analysis(
                                        &ctx_arc.cache_dir_path,
                                        &analysis,
                                    )
                                    .err()
                                } else if !solved && !quality_failure {
                                    analysis.error.clone()
                                } else {
                                    None
                                };
                            if *need_satellite && solved {
                                match runtime_handle
                                    .block_on(satellite_context.load_for_exposure(&item.fits_path))
                                {
                                    Ok(snapshot) => {
                                        if let Err(error) = crate::satellites::predict_tracks(
                                            item.image_id,
                                            &item.fits_path,
                                            &analysis,
                                            &snapshot,
                                        )
                                        .and_then(|prediction| {
                                            crate::satellites::persist_analysis(
                                                &ctx_arc.cache_dir_path,
                                                &prediction,
                                            )
                                        }) {
                                            tracing::warn!(
                                                "Satellite prediction unavailable for {}: {}",
                                                item.filename,
                                                error
                                            );
                                        }
                                    }
                                    Err(error) => tracing::warn!(
                                        "Satellite elements unavailable for {}: {}",
                                        item.filename,
                                        error
                                    ),
                                }
                            }
                            crate::server::spatial_scan::record_astrometry_result(
                                &ctx_arc.spatial_metrics,
                                &item.filename,
                                solved,
                                quality_failure,
                                operational_error,
                            );
                        }
                        Err(error) => crate::server::spatial_scan::record_astrometry_result(
                            &ctx_arc.spatial_metrics,
                            &item.filename,
                            false,
                            false,
                            Some(error),
                        ),
                    }
                }
            }
            crate::server::spatial_scan::finalize_scan(&ctx_arc.spatial_metrics);
        }));
        if scan_body.is_err() {
            tracing::error!(
                "📐 Spatial scan panicked for db={} target={}; finalizing progress",
                ctx_arc.id,
                target_id
            );
            crate::server::spatial_scan::finalize_scan(&ctx_arc.spatial_metrics);
        } else {
            tracing::info!(
                "📐 Quality scan finished for db={} target={}",
                ctx_arc.id,
                target_id
            );
        }
    });

    let (progress, cached_count) = scan::progress_snapshot(&ctx.spatial_metrics);
    Ok(Json(ApiResponse::success(SpatialScanStatusResponse {
        started: true,
        progress,
        cached_count,
    })))
}

/// GET /api/db/{db_id}/analysis/spatial-scan — progress + store size.
pub async fn get_spatial_scan_progress(
    ctx: DbContext,
) -> Result<Json<ApiResponse<SpatialScanStatusResponse>>, AppError> {
    use crate::server::spatial_scan as scan;

    scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    let (progress, cached_count) = scan::progress_snapshot(&ctx.spatial_metrics);
    Ok(Json(ApiResponse::success(SpatialScanStatusResponse {
        started: progress.running,
        progress,
        cached_count,
    })))
}

// Error handling
#[derive(Debug)]
pub enum AppError {
    NotFound,
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
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::NotFound => {
                tracing::warn!("🔍 Resource not found");
                (StatusCode::NOT_FOUND, "Resource not found")
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
