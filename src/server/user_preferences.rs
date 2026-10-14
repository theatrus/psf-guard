//! `GET/PATCH/PUT /api/me/preferences`: the signed-in user's view
//! preferences.
//!
//! The caller is the only user these endpoints name: the username comes from
//! the session or token, never from the request, so one user cannot read or
//! write another's. A viewer may save their own, since a view preference
//! changes nothing in a catalog. With no user signed in there is nothing to
//! serve; the browser keeps its preferences in its own storage then.
//!
//! The browser sends PATCH: only the keys it changed, merged into the stored
//! document, so a tab open since morning cannot undo what another browser
//! saved since. PUT, which replaces the whole document, stays for tabs still
//! running an older build.

use axum::{
    body::Bytes,
    extract::{rejection::BytesRejection, Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    server::{
        api::ApiResponse,
        auth::{RequestAccess, ServerAuth},
        handlers::{require_registry_path, AppError},
        state::AppState,
    },
    user_preferences::{
        sanitize_preference_changes, sanitize_preferences, update_file, DocumentTooLarge,
        UserPreferencesRegistry, CURRENT_SCHEMA_VERSION, MAX_REQUEST_BYTES,
    },
};

#[derive(Debug, Serialize)]
pub struct PreferencesResponse {
    pub schema_version: u32,
    pub preferences: Map<String, Value>,
    /// When they were last saved; absent for a user who has saved none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_unix_seconds: Option<i64>,
    /// Keys the request carried that this server does not know, and so did
    /// not keep.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub ignored: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavePreferencesRequest {
    schema_version: u32,
    preferences: Map<String, Value>,
}

/// How a write treats the stored document.
#[derive(Debug, Clone, Copy)]
enum Write {
    /// PUT: the request is the whole document.
    Replace,
    /// PATCH: the request's keys change those preferences; `null` resets one.
    Merge,
}

/// At startup, drop the preferences of users the server no longer has, such
/// as one removed with `psf-guard users remove` while this server ran: its
/// sessions kept working until the restart, and may have saved again.
pub fn forget_removed_users(auth: &ServerAuth, database_registry_path: &Path) {
    match crate::user_preferences::forget_unknown_users(database_registry_path, |name| {
        auth.has_user(name)
    }) {
        Ok(gone) if !gone.is_empty() => {
            tracing::info!(
                "Dropped the view preferences of removed users: {}",
                gone.join(", ")
            )
        }
        Ok(_) => {}
        Err(error) => tracing::warn!("Could not check saved view preferences: {error:#}"),
    }
}

fn signed_in(access: &RequestAccess) -> Result<String, AppError> {
    access.username.clone().ok_or_else(|| {
        AppError::NotFoundMessage(
            "No user is signed in, so preferences stay in this browser".to_string(),
        )
    })
}

fn preferences_path(state: &AppState) -> Result<PathBuf, AppError> {
    Ok(UserPreferencesRegistry::path_for_database_registry(
        &require_registry_path(state)?,
    ))
}

/// GET /api/me/preferences
pub async fn get_preferences(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
) -> Result<Json<ApiResponse<PreferencesResponse>>, AppError> {
    let username = signed_in(&access)?;
    let path = preferences_path(&state)?;
    let registry = tokio::task::spawn_blocking(move || UserPreferencesRegistry::load(&path))
        .await
        .map_err(|error| AppError::InternalError(format!("Preferences task failed: {error}")))?
        .map_err(|error| AppError::InternalError(error.to_string()))?;
    let stored = registry.get(&username);
    Ok(Json(ApiResponse::success(PreferencesResponse {
        schema_version: CURRENT_SCHEMA_VERSION,
        preferences: stored
            .map(|stored| stored.preferences.clone())
            .unwrap_or_default(),
        updated_unix_seconds: stored.map(|stored| stored.updated_unix_seconds),
        ignored: Vec::new(),
    })))
}

/// PATCH /api/me/preferences — merge changes into the caller's document.
/// Each key given replaces that preference and a key set to `null` resets
/// it; keys left out are kept. Unknown keys are dropped and named in
/// `ignored`.
pub async fn patch_preferences(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    respond(save(&state, &access, body, Write::Merge).await)
}

/// PUT /api/me/preferences — replace the caller's document. Known keys are
/// checked and kept; keys this build does not know are dropped and named in
/// `ignored`. Kept for tabs still running a build that sends whole
/// documents; current builds send PATCH.
pub async fn put_preferences(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    body: Result<Bytes, BytesRejection>,
) -> Response {
    respond(save(&state, &access, body, Write::Replace).await)
}

fn respond(result: Result<PreferencesResponse, Response>) -> Response {
    match result {
        Ok(saved) => Json(ApiResponse::success(saved)).into_response(),
        Err(response) => response,
    }
}

fn too_large() -> Response {
    (
        StatusCode::PAYLOAD_TOO_LARGE,
        Json(ApiResponse::<()>::error(format!(
            "A preferences request is limited to {MAX_REQUEST_BYTES} bytes"
        ))),
    )
        .into_response()
}

async fn save(
    state: &Arc<AppState>,
    access: &RequestAccess,
    body: Result<Bytes, BytesRejection>,
    write: Write,
) -> Result<PreferencesResponse, Response> {
    let username = signed_in(access).map_err(IntoResponse::into_response)?;
    // The route's body limit stops reading at MAX_REQUEST_BYTES; say so in
    // the API's own shape.
    let body = body.map_err(|rejection| {
        if rejection.status() == StatusCode::PAYLOAD_TOO_LARGE {
            too_large()
        } else {
            AppError::BadRequest(format!("Could not read the request: {rejection}")).into_response()
        }
    })?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(too_large());
    }
    let request: SavePreferencesRequest = serde_json::from_slice(&body).map_err(|error| {
        AppError::BadRequest(format!("Not a preferences document: {error}")).into_response()
    })?;
    if request.schema_version != CURRENT_SCHEMA_VERSION {
        return Err(AppError::BadRequest(format!(
            "Preferences schema {} is not one this server reads; it reads {}",
            request.schema_version, CURRENT_SCHEMA_VERSION
        ))
        .into_response());
    }
    let (preferences, ignored) = match write {
        Write::Replace => sanitize_preferences(&request.preferences),
        Write::Merge => sanitize_preference_changes(&request.preferences),
    }
    .map_err(|error| AppError::BadRequest(error.to_string()).into_response())?;
    let path = preferences_path(state).map_err(IntoResponse::into_response)?;
    let auth = state.server_auth();
    let worker = Arc::clone(state);
    let stored = tokio::task::spawn_blocking(move || {
        let _guard = worker.user_preferences_write.lock().unwrap();
        update_file(&path, |registry| {
            // A removed user's entry goes, and is never written back by a
            // session that outlived them.
            if let Some(auth) = &auth {
                registry.retain_users(|name| auth.has_user(name));
                if !auth.has_user(&username) {
                    return Ok(None);
                }
            }
            let now = chrono::Utc::now().timestamp();
            let stored = match write {
                Write::Replace => registry.set(&username, preferences, now)?,
                Write::Merge => registry.patch(&username, preferences, now)?,
            };
            Ok(Some(stored.clone()))
        })
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Preferences task failed: {error}")).into_response()
    })?
    .map_err(|error| {
        // A merge that would outgrow the cap is the caller's to fix.
        if error.downcast_ref::<DocumentTooLarge>().is_some() {
            AppError::BadRequest(error.to_string()).into_response()
        } else {
            AppError::InternalError(error.to_string()).into_response()
        }
    })?
    .ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(ApiResponse::<()>::error(
                "This account no longer exists".to_string(),
            )),
        )
            .into_response()
    })?;
    Ok(PreferencesResponse {
        schema_version: CURRENT_SCHEMA_VERSION,
        preferences: stored.preferences,
        updated_unix_seconds: Some(stored.updated_unix_seconds),
        ignored,
    })
}
