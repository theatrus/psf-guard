//! The HTTP side of reject removal (docs/design/reject-removal.md): preview
//! and apply a removal, list removals, restore them, and empty the trash.
//! Every route needs the server's database-management permission.

use std::{collections::BTreeSet, sync::Arc};

use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;

use crate::commands::reject_removal::{
    self, ApplyReport, PlanOptions, PurgeReport, RemovalPlan, RemovedBatch, RemovedEntry,
    RemovedFrame, RestoreReport, RestoreSelection, Scope, StaleRemovalPlan, TrashReport,
};
use crate::server::{
    api::ApiResponse,
    database_context::{open_scheduler_connection, DatabaseContext},
    extract::DbContext,
    handlers::{require_database_management_allowed, AppError},
    state::AppState,
};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewRequest {
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub target_id: Option<i64>,
    #[serde(default = "default_min_age")]
    pub min_age_days: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    #[serde(default)]
    pub project_id: Option<i64>,
    #[serde(default)]
    pub target_id: Option<i64>,
    pub min_age_days: u32,
    /// The preview's digest; Apply refuses when the rejects changed since.
    pub digest: String,
    #[serde(default = "default_retention")]
    pub retention_days: u32,
}

fn default_min_age() -> u32 {
    reject_removal::DEFAULT_MIN_AGE_DAYS
}

fn default_retention() -> u32 {
    reject_removal::DEFAULT_RETENTION_DAYS
}

#[derive(Debug, Deserialize)]
pub struct RemovedQuery {
    pub batch: Option<String>,
}

#[derive(serde::Serialize)]
pub struct RemovedView {
    pub batches: Vec<RemovedBatch>,
    /// The frames of the batch asked for; empty without one.
    pub frames: Vec<RemovedEntry>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Frames a collaboration report names: none without Director.
async fn protected(state: &AppState) -> Result<BTreeSet<String>, AppError> {
    match state.director.clone() {
        Some(service) => service.reported_image_guids().await.map_err(|error| {
            AppError::InternalError(format!("reading collaboration reports: {error}"))
        }),
        None => Ok(BTreeSet::new()),
    }
}

fn blocking_error(error: tokio::task::JoinError) -> AppError {
    AppError::InternalError(format!("reject removal: {error}"))
}

/// What a removal of this scope would take, and why each reject stays.
pub async fn preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<PreviewRequest>,
) -> Result<Json<ApiResponse<RemovalPlan>>, AppError> {
    require_database_management_allowed(&state)?;
    let protected = protected(&state).await?;
    let context = ctx.0.clone();
    let plan = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        reject_removal::plan(
            &conn,
            &context.image_dirs,
            &PlanOptions {
                scope: Scope {
                    project_id: request.project_id,
                    target_id: request.target_id,
                },
                min_age_days: request.min_age_days,
                now: now(),
                protected_guids: &protected,
            },
        )
        .map_err(|error| AppError::BadRequest(format!("{error:#}")))
    })
    .await
    .map_err(blocking_error)??;
    Ok(Json(ApiResponse::success(plan)))
}

/// Remove what the preview showed, or refuse with 409 when it changed.
pub async fn apply(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<ApplyRequest>,
) -> Result<Json<ApiResponse<ApplyReport>>, AppError> {
    require_database_management_allowed(&state)?;
    let protected = protected(&state).await?;
    let context = ctx.0.clone();
    let report = tokio::task::spawn_blocking(move || {
        let _guard = context.organization_mutex.lock().map_err(AppError::db)?;
        let conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        let report = reject_removal::apply(
            &conn,
            &context.image_dirs,
            &PlanOptions {
                scope: Scope {
                    project_id: request.project_id,
                    target_id: request.target_id,
                },
                min_age_days: request.min_age_days,
                now: now(),
                protected_guids: &protected,
            },
            &request.digest,
            request.retention_days,
        )
        .map_err(|error| {
            if error.downcast_ref::<StaleRemovalPlan>().is_some() {
                AppError::Conflict(error.to_string())
            } else {
                AppError::InternalError(format!("{error:#}"))
            }
        })?;
        after_change(&context, &conn, &report.removed);
        tracing::info!(db = %context.id, batch = %report.batch_id, removed = report.removed.len(),
            failed = report.failed.len(), "Removed rejects");
        Ok::<_, AppError>(report)
    })
    .await
    .map_err(blocking_error)??;
    if !report.removed.is_empty() {
        state.auto_stacks.touch_database(
            &ctx.id,
            crate::server::stack_preview::automatic::RefreshReason::Sync,
        );
    }
    Ok(Json(ApiResponse::success(report)))
}

/// What the server holds about frames that left or came back: their cache
/// files and pixel evidence, the file lookups, and the navigation counts.
fn after_change(context: &DatabaseContext, conn: &rusqlite::Connection, frames: &[RemovedFrame]) {
    if frames.is_empty() {
        return;
    }
    reject_removal::forget_image_caches(&context.cache_dir_path, frames);
    for frame in frames {
        if let Ok(id) = i32::try_from(frame.image_id) {
            crate::server::spatial_scan::invalidate_image_source(
                &context.spatial_metrics,
                &context.cache_dir_path,
                id,
            );
        }
    }
    context.file_check_cache.write().unwrap().clear();
    let _ = context.force_directory_tree_refresh();
    if let Err(error) = context.refresh_organization_navigation(conn) {
        tracing::warn!(db = %context.id, "Reject removal succeeded but navigation refresh failed: {error}");
    }
}

/// Every removal batch, and the frames of one when asked.
pub async fn removed(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<RemovedQuery>,
) -> Result<Json<ApiResponse<RemovedView>>, AppError> {
    require_database_management_allowed(&state)?;
    let path = ctx.database_path.clone();
    let view = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection(&path).map_err(AppError::db)?;
        let batches = reject_removal::batches(&conn).map_err(AppError::db)?;
        let frames = match &query.batch {
            Some(batch) => reject_removal::removed(&conn, Some(batch)).map_err(AppError::db)?,
            None => Vec::new(),
        };
        Ok::<_, AppError>(RemovedView { batches, frames })
    })
    .await
    .map_err(blocking_error)??;
    Ok(Json(ApiResponse::success(view)))
}

/// Put removed frames back while their files are still in the trash.
pub async fn restore(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(selection): Json<RestoreSelection>,
) -> Result<Json<ApiResponse<RestoreReport>>, AppError> {
    require_database_management_allowed(&state)?;
    if selection.batch_id.is_none() && selection.guids.is_empty() {
        return Err(AppError::BadRequest(
            "Name a batch or frames to restore.".into(),
        ));
    }
    let context = ctx.0.clone();
    let report = tokio::task::spawn_blocking(move || {
        let _guard = context.organization_mutex.lock().map_err(AppError::db)?;
        let conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        let report = reject_removal::restore(&conn, &selection)
            .map_err(|error| AppError::InternalError(format!("{error:#}")))?;
        // A restored frame can come back under a new row Id; whatever the
        // cache holds under that Id belongs to no one.
        let restored: Vec<RemovedFrame> = report
            .restored
            .iter()
            .map(|frame| RemovedFrame {
                image_id: frame.image_id,
                guid: frame.guid.clone(),
                project_id: frame.project_id,
                target_id: frame.target_id,
            })
            .collect();
        after_change(&context, &conn, &restored);
        Ok::<_, AppError>(report)
    })
    .await
    .map_err(blocking_error)??;
    if !report.restored.is_empty() {
        state.auto_stacks.touch_database(
            &ctx.id,
            crate::server::stack_preview::automatic::RefreshReason::Sync,
        );
    }
    Ok(Json(ApiResponse::success(report)))
}

/// Delete the files of removals past their retention.
pub async fn empty_trash(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<TrashReport>>, AppError> {
    require_database_management_allowed(&state)?;
    let path = ctx.database_path.clone();
    let report = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection(&path).map_err(AppError::db)?;
        reject_removal::empty_trash(&conn, now())
            .map_err(|error| AppError::InternalError(format!("{error:#}")))
    })
    .await
    .map_err(blocking_error)??;
    Ok(Json(ApiResponse::success(report)))
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PurgeRequest {
    #[serde(default)]
    pub batch_id: Option<String>,
}

/// Forget the saved rows of removals whose files are gone; markers stay.
pub async fn purge(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    request: Option<Json<PurgeRequest>>,
) -> Result<Json<ApiResponse<PurgeReport>>, AppError> {
    require_database_management_allowed(&state)?;
    let request = request.map(|Json(request)| request).unwrap_or_default();
    let path = ctx.database_path.clone();
    let report = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection(&path).map_err(AppError::db)?;
        reject_removal::purge(&conn, request.batch_id.as_deref(), now())
            .map_err(|error| AppError::InternalError(format!("{error:#}")))
    })
    .await
    .map_err(blocking_error)??;
    Ok(Json(ApiResponse::success(report)))
}
