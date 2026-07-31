//! Editor-only management of browser users.
//!
//! Session authentication stays in auth.rs. These handlers own the mutable
//! management surface and keep it separate from login and middleware.

use axum::{
    extract::{Extension, Path, State},
    Json,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc};

use crate::{
    auth_registry::{AccessRole, AuthRegistry},
    server::{
        api::ApiResponse,
        auth::{AuthUserSummary, RequestAccess, ServerAuth},
        handlers::{require_registry_path, AppError},
        state::AppState,
    },
};

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    username: String,
    role: AccessRole,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    role: AccessRole,
    #[serde(default)]
    password: Option<String>,
}

fn require_user_admin(
    state: &AppState,
    access: RequestAccess,
) -> Result<(ServerAuth, PathBuf, Option<String>), AppError> {
    if access.role != AccessRole::ReadWrite {
        return Err(AppError::Forbidden(
            "Only an editor can manage browser users".to_string(),
        ));
    }
    let auth = state.server_auth().ok_or_else(|| {
        AppError::Forbidden("Browser authentication is not enabled on this server".to_string())
    })?;
    let database_registry_path = require_registry_path(state)?;
    let auth_registry_path = AuthRegistry::path_for_database_registry(&database_registry_path);
    Ok((auth, auth_registry_path, access.username))
}

pub async fn list_users(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
) -> Result<Json<ApiResponse<Vec<AuthUserSummary>>>, AppError> {
    let (auth, _, _) = require_user_admin(&state, access)?;
    Ok(Json(ApiResponse::success(auth.user_summaries())))
}

pub async fn create_user(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Json(request): Json<CreateUserRequest>,
) -> Result<Json<ApiResponse<Vec<AuthUserSummary>>>, AppError> {
    let (auth, registry_path, _) = require_user_admin(&state, access)?;
    let worker = auth.clone();
    tokio::task::spawn_blocking(move || {
        worker.add_user(
            &registry_path,
            &request.username,
            request.role,
            &request.password,
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("User update task failed: {error}")))?
    .map_err(|error| AppError::BadRequest(error.to_string()))?;
    Ok(Json(ApiResponse::success(auth.user_summaries())))
}

pub async fn update_user(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Path(username): Path<String>,
    Json(request): Json<UpdateUserRequest>,
) -> Result<Json<ApiResponse<Vec<AuthUserSummary>>>, AppError> {
    let (auth, registry_path, _) = require_user_admin(&state, access)?;
    let worker = auth.clone();
    let stored_username = username.clone();
    tokio::task::spawn_blocking(move || {
        worker.update_user(
            &registry_path,
            &stored_username,
            request.role,
            request.password.as_deref(),
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("User update task failed: {error}")))?
    .map_err(|error| AppError::BadRequest(error.to_string()))?;
    Ok(Json(ApiResponse::success(auth.user_summaries())))
}

pub async fn remove_user(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Path(username): Path<String>,
) -> Result<Json<ApiResponse<Vec<AuthUserSummary>>>, AppError> {
    let (auth, registry_path, current_username) = require_user_admin(&state, access)?;
    if current_username.as_deref() == Some(username.as_str()) {
        return Err(AppError::BadRequest(
            "You cannot remove the account used by this session".to_string(),
        ));
    }
    let worker = auth.clone();
    let stored_username = username.clone();
    tokio::task::spawn_blocking(move || worker.remove_user(&registry_path, &stored_username))
        .await
        .map_err(|error| AppError::InternalError(format!("User update task failed: {error}")))?
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    Ok(Json(ApiResponse::success(auth.user_summaries())))
}
