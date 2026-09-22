//! The AstroBin acquisition CSV: a per-target or per-project export, and
//! the filter-id settings it needs.
//!
//! The export is read-only and small, so both routes answer in the request
//! (on a blocking thread with their own read-only connection, as the zip
//! export does). The JSON route feeds the dialog's preview; the `.csv`
//! route is the download link. The settings block lives in the registry
//! with the other process-global preferences: GET is open to viewers so
//! the dialog can show which filters still need an id, PUT lands in
//! `requires_write`.

use axum::{
    extract::{Query, State},
    http::header,
    Json,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::Arc;

use crate::astrobin::{AstroBinDetail, AstroBinExport, AstroBinExportRequest};
use crate::db_registry::{AstroBinSettings, DbRegistry};
use crate::server::{
    api::ApiResponse,
    database_context::open_scheduler_connection_with_flags,
    extract::DbContext,
    handlers::{require_registry_path, AppError},
    state::AppState,
};

#[derive(Debug, Serialize, Deserialize)]
pub struct AstroBinSettingsResponse {
    /// Filter name to AstroBin equipment id. Never absent: an unconfigured
    /// registry has an empty map.
    pub filter_ids: BTreeMap<String, u32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateAstroBinSettingsRequest {
    /// The whole map; an entry left out is forgotten.
    pub filter_ids: BTreeMap<String, u32>,
}

fn filter_ids(state: &AppState) -> Result<BTreeMap<String, u32>, AppError> {
    // A server without a persistent registry has no ids to offer; the
    // export still works, with blank filter cells.
    let Ok(path) = require_registry_path(state) else {
        return Ok(BTreeMap::new());
    };
    let registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(registry
        .astrobin
        .map(|settings| settings.filter_ids)
        .unwrap_or_default())
}

/// GET /api/settings/astrobin
pub async fn get_astrobin_settings(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<AstroBinSettingsResponse>>, AppError> {
    Ok(Json(ApiResponse::success(AstroBinSettingsResponse {
        filter_ids: filter_ids(&state)?,
    })))
}

/// PUT /api/settings/astrobin
pub async fn update_astrobin_settings(
    State(state): State<Arc<AppState>>,
    Json(request): Json<UpdateAstroBinSettingsRequest>,
) -> Result<Json<ApiResponse<AstroBinSettingsResponse>>, AppError> {
    let path = require_registry_path(&state)?;
    let _registry_guard = state.registry_write.lock().await;
    let mut registry = DbRegistry::load_or_init(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let filter_ids: BTreeMap<String, u32> = request
        .filter_ids
        .into_iter()
        .map(|(name, id)| (name.trim().to_string(), id))
        .filter(|(name, id)| !name.is_empty() && *id > 0)
        .collect();
    // An empty map is what an absent block already means; storing nothing
    // keeps the registry clean for older builds reading the same file.
    registry.astrobin = if filter_ids.is_empty() {
        None
    } else {
        Some(AstroBinSettings {
            filter_ids: filter_ids.clone(),
        })
    };
    registry
        .save(&path)
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(Json(ApiResponse::success(AstroBinSettingsResponse {
        filter_ids,
    })))
}

#[derive(Debug, Deserialize)]
pub struct AstroBinExportQuery {
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    /// Count ungraded lights too. Rejects never count.
    #[serde(default)]
    pub include_pending: bool,
    #[serde(default)]
    pub detail: AstroBinDetail,
}

async fn build(
    state: &AppState,
    ctx: &DbContext,
    query: AstroBinExportQuery,
) -> Result<AstroBinExport, AppError> {
    if query.project_id.is_none() && query.target_id.is_none() {
        return Err(AppError::BadRequest(
            "name a target_id or a project_id to export".into(),
        ));
    }
    let filter_ids = filter_ids(state)?;
    let request = AstroBinExportRequest {
        project_id: query.project_id,
        target_id: query.target_id,
        include_pending: query.include_pending,
        detail: query.detail,
    };
    // Only full detail reads frames from disk, so only it needs the
    // folder index; essentials answers from the catalog alone.
    let directory_tree = match query.detail {
        AstroBinDetail::Full => Some(ctx.get_directory_tree().map_err(|error| {
            AppError::InternalError(format!("indexing image folders: {error}"))
        })?),
        AstroBinDetail::Essentials => None,
    };
    let database_path = ctx.database_path.clone();
    tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| anyhow::anyhow!("opening {database_path}: {error}"))?;
        crate::astrobin::export(&conn, directory_tree.as_deref(), &filter_ids, &request)
    })
    .await
    .map_err(|error| AppError::InternalError(format!("export task: {error}")))?
    .map_err(|error| AppError::InternalError(format!("AstroBin export: {error:#}")))
}

/// `GET /api/db/{db_id}/astrobin-export` — the rows, the CSV, and what
/// still needs a filter id, for the dialog.
pub async fn get_astrobin_export(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<AstroBinExportQuery>,
) -> Result<Json<ApiResponse<AstroBinExport>>, AppError> {
    let export = build(&state, &ctx, query).await?;
    Ok(Json(ApiResponse::success(export)))
}

/// `GET /api/db/{db_id}/astrobin-export.csv` — the same export as a file.
pub async fn get_astrobin_csv(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<AstroBinExportQuery>,
) -> Result<axum::response::Response, AppError> {
    let export = build(&state, &ctx, query).await?;
    axum::response::Response::builder()
        .header(header::CONTENT_TYPE, "text/csv; charset=utf-8")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", export.filename),
        )
        .body(axum::body::Body::from(export.csv))
        .map_err(|error| AppError::InternalError(format!("building response: {error}")))
}
