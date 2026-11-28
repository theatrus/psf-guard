//! `/api/db/{db}/studio-documents`: PSF Studio documents saved to a
//! database's projects, the files they use, and their exports. See
//! `crate::studio_documents` for what is kept and where.
//!
//! A Studio token may make these writes when its user is an editor; the
//! auth middleware checks that before a handler runs.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Extension, Json,
};
use http_body_util::BodyExt;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::auth_registry::unix_now;
use crate::server::api::ApiResponse;
use crate::server::auth::RequestAccess;
use crate::server::extract::DbContext;
use crate::server::handlers::{require_registry_path, AppError};
use crate::server::state::AppState;
use crate::studio_documents::{
    NewDocument, NewExport, Save, Saved, StudioDocuments, MAX_FILE_BYTES,
};

fn store(state: &AppState, database: &str) -> Result<StudioDocuments, AppError> {
    let registry = require_registry_path(state)?;
    StudioDocuments::for_database(&registry, database)
        .map_err(|error| AppError::BadRequest(error.to_string()))
}

/// Who saves: the signed-in user, or "this computer" on a server without
/// accounts. PSF Studio and the project page read that value to word such
/// a save by the computer it came from instead.
fn saved_by(access: Option<Extension<RequestAccess>>) -> String {
    access
        .and_then(|Extension(access)| access.username)
        .unwrap_or_else(|| "this computer".to_string())
}

/// Runs store work on the blocking pool, one write at a time per server.
async fn blocking<T: Send + 'static>(
    state: &Arc<AppState>,
    work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
) -> Result<T, AppError> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || {
        let _one_at_a_time = state.studio_documents_write.lock().unwrap();
        work()
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Studio document task failed: {error}")))?
    .map_err(|error| AppError::BadRequest(error.to_string()))
}

#[derive(Debug, Default, Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub project: Option<i32>,
    #[serde(default)]
    pub target: Option<i32>,
}

/// `GET /studio-documents?project=&target=`
pub async fn list_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<ListQuery>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    let list = blocking(&state, move || store.list(query.project, query.target)).await?;
    Ok(Json(ApiResponse::success(list)).into_response())
}

/// `POST /studio-documents`: a new document and its first revision.
pub async fn create_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    access: Option<Extension<RequestAccess>>,
    Json(new): Json<NewDocument>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    let by = saved_by(access);
    let record = blocking(&state, move || store.create(new, &by, unix_now())).await?;
    Ok(Json(ApiResponse::success(record)).into_response())
}

/// `GET /studio-documents/{id}`: the record, with its revision list.
pub async fn record_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    match blocking(&state, move || store.record(&id)).await? {
        Some(record) => Ok(Json(ApiResponse::success(record)).into_response()),
        None => Err(AppError::NotFound),
    }
}

/// `GET /studio-documents/{id}/revisions/{revision}`: the document itself.
pub async fn revision_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, id, revision)): Path<(String, String, u32)>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    match blocking(&state, move || store.revision(&id, revision)).await? {
        Some(text) => Ok(([(header::CONTENT_TYPE, "application/json")], text).into_response()),
        None => Err(AppError::NotFound),
    }
}

/// `GET /studio-documents/{id}/processing`: the current revision as
/// PSF Guard's color processing settings, which a Color processing setup
/// keeps. A document with a step PSF Guard's pipeline does not run is
/// refused, with the reason.
pub async fn processing_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    let text = blocking(&state, move || {
        let Some(record) = store.record(&id)? else {
            return Ok(None);
        };
        store.revision(&id, record.current().revision)
    })
    .await?
    .ok_or(AppError::NotFound)?;
    let document = crate::studio::Document::from_json(&text)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    let processing = crate::studio_flow::processing_from_document(&document).map_err(|error| {
        AppError::BadRequest(format!(
            "This document cannot be a Color processing setup: {error}"
        ))
    })?;
    Ok(Json(ApiResponse::success(processing)).into_response())
}

/// `PUT /studio-documents/{id}`: a new revision. When another save arrived
/// first, 409 with the record as it is.
pub async fn save_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    access: Option<Extension<RequestAccess>>,
    Path((_db, id)): Path<(String, String)>,
    Json(save): Json<Save>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    let by = saved_by(access);
    match blocking(&state, move || store.save(&id, save, &by, unix_now())).await? {
        Some(Saved::Saved(record)) => Ok(Json(ApiResponse::success(record)).into_response()),
        Some(Saved::Conflict(record)) => Ok((
            StatusCode::CONFLICT,
            Json(ApiResponse {
                success: false,
                error: Some(format!(
                    "Revision {} was saved after the one this save started from",
                    record.current().revision
                )),
                data: Some(record),
                status: None,
            }),
        )
            .into_response()),
        None => Err(AppError::NotFound),
    }
}

/// `POST /studio-documents/{id}/exports`: records an uploaded export.
pub async fn export_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    access: Option<Extension<RequestAccess>>,
    Path((_db, id)): Path<(String, String)>,
    Json(export): Json<NewExport>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    let by = saved_by(access);
    match blocking(&state, move || {
        store.add_export(&id, export, &by, unix_now())
    })
    .await?
    {
        Some(record) => Ok(Json(ApiResponse::success(record)).into_response()),
        None => Err(AppError::NotFound),
    }
}

/// `DELETE /studio-documents/{id}`: the document, its revisions, and the
/// files only it uses.
pub async fn delete_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, id)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    if blocking(&state, move || store.delete(&id, unix_now())).await? {
        Ok(Json(ApiResponse::success(())).into_response())
    } else {
        Err(AppError::NotFound)
    }
}

/// `HEAD` or `GET /studio-documents/files/{sha256}`.
pub async fn file_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, sha)): Path<(String, String)>,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    let path = store
        .file_path(&sha)
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| AppError::NotFound)?;
    let size = file
        .metadata()
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?
        .len();
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable".to_string(),
            ),
        ],
        crate::server::file_body::file_body(file),
    )
        .into_response())
}

/// `PUT /studio-documents/files/{sha256}`: uploads a file, which is kept
/// only when its bytes have that hash.
pub async fn upload_route(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, sha)): Path<(String, String)>,
    body: Body,
) -> Result<Response, AppError> {
    let store = store(&state, &ctx.id)?;
    if store
        .has_file(&sha)
        .map_err(|error| AppError::BadRequest(error.to_string()))?
    {
        return Ok(Json(ApiResponse::success(())).into_response());
    }
    let incoming = store
        .incoming_file()
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let written = write_upload(body, &incoming).await;
    let hash = match written {
        Ok(hash) => hash,
        Err(error) => {
            let _ = tokio::fs::remove_file(&incoming).await;
            return Err(error);
        }
    };
    if hash != sha {
        let _ = tokio::fs::remove_file(&incoming).await;
        return Err(AppError::BadRequest(format!(
            "The file's SHA-256 is {hash}, not {sha}"
        )));
    }
    blocking(&state, move || store.keep_file(&incoming, &sha)).await?;
    Ok(Json(ApiResponse::success(())).into_response())
}

/// Writes a request body to `path`, and answers its SHA-256.
async fn write_upload(body: Body, path: &std::path::Path) -> Result<String, AppError> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    let mut body = body;
    while let Some(frame) = body.frame().await {
        let frame =
            frame.map_err(|error| AppError::BadRequest(format!("The upload stopped: {error}")))?;
        let Ok(bytes) = frame.into_data() else {
            continue;
        };
        size += bytes.len() as u64;
        if size > MAX_FILE_BYTES {
            return Err(AppError::BadRequest(format!(
                "A file may be at most {} GB",
                MAX_FILE_BYTES / 1024 / 1024 / 1024
            )));
        }
        hasher.update(&bytes);
        file.write_all(&bytes)
            .await
            .map_err(|error| AppError::InternalError(error.to_string()))?;
    }
    file.sync_all()
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
