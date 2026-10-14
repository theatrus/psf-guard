//! `GET/PUT /api/me/preferences`: the signed-in user's view preferences.
//!
//! The caller is the only user these endpoints name: the username comes from
//! the session or token, never from the request, so one user cannot read or
//! write another's. A viewer may save their own, since a view preference
//! changes nothing in a catalog. With no user signed in there is nothing to
//! serve; the browser keeps its preferences in its own storage then.

use axum::{
    body::Bytes,
    extract::{Extension, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{path::PathBuf, sync::Arc};

use crate::{
    server::{
        api::ApiResponse,
        auth::RequestAccess,
        handlers::{require_registry_path, AppError},
        state::AppState,
    },
    user_preferences::{
        sanitize_preferences, UserPreferencesRegistry, CURRENT_SCHEMA_VERSION, MAX_REQUEST_BYTES,
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

/// PUT /api/me/preferences — replace the caller's document. Known keys are
/// checked and kept; keys this build does not know are dropped and named in
/// `ignored`.
pub async fn put_preferences(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    body: Bytes,
) -> Response {
    match save(&state, &access, body).await {
        Ok(saved) => Json(ApiResponse::success(saved)).into_response(),
        Err(response) => response,
    }
}

async fn save(
    state: &Arc<AppState>,
    access: &RequestAccess,
    body: Bytes,
) -> Result<PreferencesResponse, Response> {
    let username = signed_in(access).map_err(IntoResponse::into_response)?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ApiResponse::<()>::error(format!(
                "A preferences request is limited to {MAX_REQUEST_BYTES} bytes"
            ))),
        )
            .into_response());
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
    let (preferences, ignored) = sanitize_preferences(&request.preferences)
        .map_err(|error| AppError::BadRequest(error.to_string()).into_response())?;
    let path = preferences_path(state).map_err(IntoResponse::into_response)?;
    let worker = Arc::clone(state);
    let stored = tokio::task::spawn_blocking(move || {
        let _guard = worker.user_preferences_write.lock().unwrap();
        let mut registry = UserPreferencesRegistry::load(&path)?;
        let stored = registry
            .set(&username, preferences, chrono::Utc::now().timestamp())?
            .clone();
        registry.save(&path)?;
        anyhow::Ok(stored)
    })
    .await
    .map_err(|error| {
        AppError::InternalError(format!("Preferences task failed: {error}")).into_response()
    })?
    .map_err(|error| AppError::InternalError(error.to_string()).into_response())?;
    Ok(PreferencesResponse {
        schema_version: CURRENT_SCHEMA_VERSION,
        preferences: stored.preferences,
        updated_unix_seconds: Some(stored.updated_unix_seconds),
        ignored,
    })
}
