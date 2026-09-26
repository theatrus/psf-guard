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
        auth::{AuthTokenSummary, AuthUserSummary, MintedToken, RequestAccess, ServerAuth},
        handlers::{require_registry_path, AppError},
        state::AppState,
    },
};

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    username: String,
    role: AccessRole,
    #[serde(default)]
    email: Option<String>,
    password: String,
}

#[derive(Debug, Deserialize)]
pub struct UpdateUserRequest {
    role: AccessRole,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CreateTokenRequest {
    label: String,
    #[serde(default)]
    read_only: bool,
    #[serde(default)]
    expires_in_days: Option<u32>,
    /// Mint for another user. Only an editor may set this.
    #[serde(default)]
    username: Option<String>,
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
            request.email.as_deref(),
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
            request.email.as_deref(),
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

/// Token management needs a signed-in session. A token that could mint
/// tokens would never expire in practice.
fn require_token_session(
    state: &AppState,
    access: RequestAccess,
) -> Result<(ServerAuth, PathBuf, String, AccessRole), AppError> {
    if access.api_token {
        return Err(AppError::Forbidden(
            "Sign in with a browser session to manage API tokens".to_string(),
        ));
    }
    let auth = state.server_auth().ok_or_else(|| {
        AppError::Forbidden(
            "API tokens need user accounts; this server has none, so it serves its own machine \
             without them"
                .to_string(),
        )
    })?;
    let username = access.username.ok_or_else(|| {
        AppError::Forbidden("Sign in with a browser session to manage API tokens".to_string())
    })?;
    let database_registry_path = require_registry_path(state)?;
    let auth_registry_path = AuthRegistry::path_for_database_registry(&database_registry_path);
    Ok((auth, auth_registry_path, username, access.role))
}

/// An editor sees every token; a viewer sees their own.
fn token_scope(role: AccessRole, username: &str) -> Option<&str> {
    (role != AccessRole::ReadWrite).then_some(username)
}

pub async fn list_tokens(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
) -> Result<Json<ApiResponse<Vec<AuthTokenSummary>>>, AppError> {
    let (auth, _, username, role) = require_token_session(&state, access)?;
    Ok(Json(ApiResponse::success(
        auth.token_summaries(token_scope(role, &username)),
    )))
}

pub async fn create_token(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Json(request): Json<CreateTokenRequest>,
) -> Result<Json<ApiResponse<MintedToken>>, AppError> {
    let (auth, registry_path, username, role) = require_token_session(&state, access)?;
    let owner = match request.username {
        Some(other) if other != username => {
            if role != AccessRole::ReadWrite {
                return Err(AppError::Forbidden(
                    "Only an editor can mint a token for another user".to_string(),
                ));
            }
            other
        }
        _ => username.clone(),
    };
    let worker = auth.clone();
    let mut minted = tokio::task::spawn_blocking(move || {
        worker.create_token(
            &registry_path,
            &owner,
            &request.label,
            request.read_only,
            request.expires_in_days,
        )
    })
    .await
    .map_err(|error| AppError::InternalError(format!("Token task failed: {error}")))?
    .map_err(|error| AppError::BadRequest(error.to_string()))?;
    minted.tokens = auth.token_summaries(token_scope(role, &username));
    Ok(Json(ApiResponse::success(minted)))
}

pub async fn revoke_token(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<Vec<AuthTokenSummary>>>, AppError> {
    let (auth, registry_path, username, role) = require_token_session(&state, access)?;
    let worker = auth.clone();
    let owner = token_scope(role, &username).map(str::to_string);
    tokio::task::spawn_blocking(move || worker.revoke_token(&registry_path, &id, owner.as_deref()))
        .await
        .map_err(|error| AppError::InternalError(format!("Token task failed: {error}")))?
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    Ok(Json(ApiResponse::success(
        auth.token_summaries(token_scope(role, &username)),
    )))
}
