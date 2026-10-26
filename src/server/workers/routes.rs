//! The workers' HTTP routes, under `/api/workers/v1/`, and Settings ›
//! Workers' routes, under `/api/workers`.
//!
//! The workers' routes skip the user sign-in: each request carries a worker
//! token (`Authorization: Bearer psfw_…`), and a token reaches only the
//! sessions registered with it and the tasks those sessions hold.

use std::sync::Arc;

use axum::{
    body::Body,
    extract::{Path, State},
    http::{header::AUTHORIZATION, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Extension, Json, Router,
};
use http_body_util::BodyExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use super::board::{QueuedSummary, Refusal, SessionSummary, WorkPlacement, POLL};
use crate::remote_work::protocol::{
    valid_file_name, Completion, Failure, ProbeAnswer, Progress, Registered, Registration,
    TaskKind, DIGEST_HEADER, FRAMES_HEADER, LEASE_HEADER, PROTOCOL_VERSION,
};
use crate::server::api::ApiResponse;
use crate::server::auth::{AccessRole, RequestAccess};
use crate::server::handlers::{require_database_management_allowed, update_registry, AppError};
use crate::server::state::AppState;

/// The largest output a worker may upload.
const MAX_OUTPUT_BYTES: u64 = 16 * 1024 * 1024 * 1024;
/// The largest progress report a worker may send.
const MAX_PROGRESS_BYTES: usize = 64 * 1024 * 1024;
/// The largest running image a worker may send. Workers send images of
/// about one megapixel.
const MAX_PREVIEW_BYTES: usize = 16 * 1024 * 1024;
/// The widest or tallest running image a worker may send.
const MAX_PREVIEW_DIMENSION: u32 = 4096;

/// The routes workers call, nested under `/api/workers/v1`.
pub fn worker_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/register", post(register))
        .route("/sessions/{session}/probe", post(answer_probe))
        .route("/sessions/{session}/claim", post(claim))
        .route("/sessions/{session}/leave", post(leave))
        .route(
            "/sessions/{session}/tasks/{task}/progress",
            // A report can carry a stack's whole checkpoint ledger.
            post(progress).layer(axum::extract::DefaultBodyLimit::max(MAX_PROGRESS_BYTES)),
        )
        .route(
            "/sessions/{session}/tasks/{task}/preview",
            put(preview).layer(axum::extract::DefaultBodyLimit::max(MAX_PREVIEW_BYTES)),
        )
        .route(
            "/sessions/{session}/tasks/{task}/inputs/{name}",
            get(download_input),
        )
        .route(
            "/sessions/{session}/tasks/{task}/outputs/{name}",
            put(upload_output),
        )
        .route("/sessions/{session}/tasks/{task}/complete", post(complete))
        .route("/sessions/{session}/tasks/{task}/fail", post(fail))
}

/// Settings › Workers' routes, merged into `/api`.
pub fn settings_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/workers", get(list))
        .route("/workers/tokens", post(create_token))
        .route("/workers/tokens/{id}", delete(revoke_token))
        .route("/workers/exchange", put(set_exchange))
        .route("/workers/placement", put(set_placement))
}

fn refusal(status: StatusCode, message: impl Into<String>) -> Response {
    (status, Json(ApiResponse::<()>::error(message.into()))).into_response()
}

impl IntoResponse for Refusal {
    fn into_response(self) -> Response {
        let status = match self {
            Refusal::UnknownSession => StatusCode::NOT_FOUND,
            Refusal::NotCurrent => StatusCode::CONFLICT,
            Refusal::Invalid(_) => StatusCode::BAD_REQUEST,
        };
        refusal(status, self.to_string())
    }
}

/// The worker token a request carries, as the id of a current token.
#[allow(clippy::result_large_err)]
fn token(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    let denied = || {
        refusal(
            StatusCode::UNAUTHORIZED,
            "The worker token is not valid; create a new one in Settings › Workers",
        )
    };
    let secret = headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, secret)| secret.trim())
        .ok_or_else(denied)?;
    state.workers.token_id(secret).ok_or_else(denied)
}

/// Check that the request's token registered `session`.
#[allow(clippy::result_large_err)]
fn session_of_token(state: &AppState, headers: &HeaderMap, session: &str) -> Result<(), Response> {
    let token = token(state, headers)?;
    match state.workers.board.token_of(session) {
        Some(owner) if owner == token => Ok(()),
        _ => Err(Refusal::UnknownSession.into_response()),
    }
}

#[allow(clippy::result_large_err)]
fn lease(headers: &HeaderMap) -> Result<String, Response> {
    headers
        .get(LEASE_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .ok_or_else(|| {
            refusal(
                StatusCode::BAD_REQUEST,
                format!("The request has no lease; send it in {LEASE_HEADER}"),
            )
        })
}

/// POST /api/workers/v1/register
async fn register(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(mut registration): Json<Registration>,
) -> Response {
    let token = match token(&state, &headers) {
        Ok(token) => token,
        Err(response) => return response,
    };
    if registration.protocol != PROTOCOL_VERSION {
        return refusal(
            StatusCode::BAD_REQUEST,
            format!(
                "This server speaks worker protocol {PROTOCOL_VERSION} (PSF Guard {}); the worker \
                 speaks {}. Install the same PSF Guard release on both.",
                env!("CARGO_PKG_VERSION"),
                registration.protocol
            ),
        );
    }
    registration.name = match super::tokens::normalize_name(&registration.name) {
        Ok(name) => name,
        Err(message) => return refusal(StatusCode::BAD_REQUEST, message),
    };
    let name = registration.name.clone();
    let board = Arc::clone(&state.workers.board);
    let session = match tokio::task::spawn_blocking(move || {
        board.register(&token, registration, false)
    })
    .await
    {
        Ok(session) => session,
        Err(error) => return refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    };
    tracing::info!(worker = name, "A worker joined");
    Json(Registered {
        probe: state.workers.board.probe_of(&session),
        session,
        lease_seconds: state.workers.board.lease().as_secs(),
        poll_seconds: POLL.as_secs(),
    })
    .into_response()
}

/// POST /api/workers/v1/sessions/{session}/probe
async fn answer_probe(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session): Path<String>,
    Json(answer): Json<ProbeAnswer>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let board = Arc::clone(&state.workers.board);
    match tokio::task::spawn_blocking(move || board.answer_probe(&session, answer)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(refused)) => refused.into_response(),
        Err(error) => refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// POST /api/workers/v1/sessions/{session}/claim: a task, or `204` once no
/// task was ready for the poll time.
async fn claim(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session): Path<String>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    match state.workers.board.claim(&session, POLL).await {
        Ok(Some(assignment)) => Json(assignment).into_response(),
        Ok(None) => StatusCode::NO_CONTENT.into_response(),
        Err(refused) => refused.into_response(),
    }
}

/// POST /api/workers/v1/sessions/{session}/leave
async fn leave(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session): Path<String>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    state.workers.board.leave(&session);
    StatusCode::NO_CONTENT.into_response()
}

/// POST …/tasks/{task}/progress
async fn progress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session, task)): Path<(String, String)>,
    Json(progress): Json<Progress>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let lease = match lease(&headers) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    match state
        .workers
        .board
        .progress(&session, &task, &lease, progress)
    {
        Ok(reply) => Json(reply).into_response(),
        Err(refused) => refused.into_response(),
    }
}

/// PUT …/tasks/{task}/preview: a running image of the task's work, as a
/// PNG, with the number of frames it integrates in [`FRAMES_HEADER`].
async fn preview(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session, task)): Path<(String, String)>,
    png: axum::body::Bytes,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let lease = match lease(&headers) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let Some(frames) = headers
        .get(FRAMES_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u32>().ok())
    else {
        return refusal(
            StatusCode::BAD_REQUEST,
            format!("The image has no {FRAMES_HEADER} header"),
        );
    };
    match png_dimensions(&png) {
        Some((width, height))
            if width <= MAX_PREVIEW_DIMENSION && height <= MAX_PREVIEW_DIMENSION => {}
        Some(_) => {
            return refusal(
                StatusCode::BAD_REQUEST,
                format!("The image is larger than {MAX_PREVIEW_DIMENSION} pixels a side"),
            )
        }
        None => return refusal(StatusCode::BAD_REQUEST, "The image is not a PNG"),
    }
    let preview = super::TaskPreview { png, frames };
    match state
        .workers
        .board
        .preview(&session, &task, &lease, preview)
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(refused) => refused.into_response(),
    }
}

/// A PNG's width and height, from its header.
fn png_dimensions(png: &[u8]) -> Option<(u32, u32)> {
    if !png.starts_with(b"\x89PNG\r\n\x1a\n") || png.get(12..16)? != b"IHDR" {
        return None;
    }
    let number = |at: usize| Some(u32::from_be_bytes(png.get(at..at + 4)?.try_into().ok()?));
    Some((number(16)?, number(20)?))
}

/// GET …/tasks/{task}/inputs/{name}
async fn download_input(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session, task, name)): Path<(String, String, String)>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let lease = match lease(&headers) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let path = match state.workers.board.input(&session, &task, &lease, &name) {
        Ok(path) => path,
        Err(refused) => return refused.into_response(),
    };
    let file = match tokio::fs::File::open(&path).await {
        Ok(file) => file,
        Err(error) => {
            return refusal(
                StatusCode::GONE,
                format!("The input {name} could not be read: {error}"),
            )
        }
    };
    let length = file.metadata().await.map(|metadata| metadata.len()).ok();
    let mut response = crate::server::file_body::file_body(file).into_response();
    if let Some(length) = length {
        response
            .headers_mut()
            .insert(axum::http::header::CONTENT_LENGTH, length.into());
    }
    response
}

/// PUT …/tasks/{task}/outputs/{name}: the output's bytes, checked against
/// the digest header before the file is renamed to the output's name.
async fn upload_output(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session, task, name)): Path<(String, String, String)>,
    body: Body,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let lease = match lease(&headers) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    if !valid_file_name(&name) {
        return refusal(StatusCode::BAD_REQUEST, "The output's name is not valid");
    }
    let Some(expected) = headers
        .get(DIGEST_HEADER)
        .and_then(|value| value.to_str().ok())
        .map(str::to_ascii_lowercase)
    else {
        return refusal(StatusCode::BAD_REQUEST, "The upload has no digest");
    };
    let path = match state
        .workers
        .board
        .output_path(&session, &task, &lease, &name)
    {
        Ok(path) => path,
        Err(refused) => return refused.into_response(),
    };
    let random: [u8; 8] = rand::random();
    let temporary = path.with_file_name(format!(
        ".{name}.{}.upload",
        crate::remote_work::hex(&random)
    ));
    match receive(body, &temporary).await {
        Ok(digest) if digest == expected => {}
        Ok(_) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return refusal(
                StatusCode::BAD_REQUEST,
                format!("The upload of {name} does not match its digest"),
            );
        }
        Err(message) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return refusal(StatusCode::BAD_REQUEST, message);
        }
    }
    // The lease may have run out while the file arrived.
    if let Err(refused) = state
        .workers
        .board
        .output_path(&session, &task, &lease, &name)
    {
        let _ = tokio::fs::remove_file(&temporary).await;
        return refused.into_response();
    }
    if let Err(error) = tokio::fs::rename(&temporary, &path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string());
    }
    StatusCode::NO_CONTENT.into_response()
}

/// Write `body` to `path`, and return its SHA-256.
async fn receive(mut body: Body, path: &std::path::Path) -> Result<String, String> {
    let mut file = tokio::fs::File::create(path)
        .await
        .map_err(|error| format!("The output could not be stored: {error}"))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    while let Some(frame) = body.frame().await {
        let frame = frame.map_err(|error| format!("The upload broke off: {error}"))?;
        let Ok(data) = frame.into_data() else {
            continue;
        };
        size += data.len() as u64;
        if size > MAX_OUTPUT_BYTES {
            return Err("The output is larger than a worker may upload".into());
        }
        hasher.update(&data);
        file.write_all(&data)
            .await
            .map_err(|error| format!("The output could not be stored: {error}"))?;
    }
    file.flush()
        .await
        .map_err(|error| format!("The output could not be stored: {error}"))?;
    Ok(crate::remote_work::hex(&hasher.finalize()))
}

/// POST …/tasks/{task}/complete
async fn complete(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session, task)): Path<(String, String)>,
    Json(completion): Json<Completion>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let lease = match lease(&headers) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    let board = Arc::clone(&state.workers.board);
    // Checking the outputs reads every byte of them.
    match tokio::task::spawn_blocking(move || board.complete(&session, &task, &lease, completion))
        .await
    {
        Ok(Ok(())) => StatusCode::NO_CONTENT.into_response(),
        Ok(Err(refused)) => refused.into_response(),
        Err(error) => refusal(StatusCode::INTERNAL_SERVER_ERROR, error.to_string()),
    }
}

/// POST …/tasks/{task}/fail
async fn fail(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session, task)): Path<(String, String)>,
    Json(failure): Json<Failure>,
) -> Response {
    if let Err(response) = session_of_token(&state, &headers, &session) {
        return response;
    }
    let lease = match lease(&headers) {
        Ok(lease) => lease,
        Err(response) => return response,
    };
    match state
        .workers
        .board
        .fail(&session, &task, &lease, failure.error)
    {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(refused) => refused.into_response(),
    }
}

/// A worker token, without its secret.
#[derive(Debug, Clone, Serialize)]
pub struct TokenSummary {
    pub id: String,
    pub name: String,
    pub created_at: i64,
}

/// `GET /api/workers`
#[derive(Debug, Clone, Serialize)]
pub struct WorkersResponse {
    pub tokens: Vec<TokenSummary>,
    /// This server's worker first, then the others by name.
    pub workers: Vec<SessionSummary>,
    /// Tasks waiting for a worker, the next first.
    pub queued: Vec<QueuedSummary>,
    /// The folder task files go through now.
    pub exchange_folder: String,
    /// The folder chosen in Settings, when one was.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange_folder_setting: Option<String>,
    /// Where task files go when no folder is chosen.
    pub default_exchange_folder: String,
    /// The folder inside the exchange folder that holds task files.
    pub exchange_subfolder: &'static str,
    /// Where each kind of work runs.
    pub placement: Placement,
}

/// Where each kind of work runs. WBPP runs stay on this server.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Placement {
    pub rc_astro: WorkPlacement,
    /// Omitted keeps the current choice.
    #[serde(default)]
    pub stack: Option<WorkPlacement>,
}

/// `POST /api/workers/tokens`
#[derive(Debug, Clone, Deserialize)]
pub struct CreateTokenRequest {
    pub name: String,
}

/// A new token. `token` is the only time the secret leaves the server.
#[derive(Debug, Clone, Serialize)]
pub struct CreatedToken {
    pub token: String,
    pub summary: TokenSummary,
    pub workers: WorkersResponse,
}

/// `PUT /api/workers/exchange`
#[derive(Debug, Clone, Deserialize)]
pub struct ExchangeRequest {
    /// An absolute folder, or `None` for the default.
    pub folder: Option<String>,
}

/// Settings › Workers is an editor's, on a server that allows database
/// management: a worker token lets a machine read the files of the tasks it
/// runs, and the exchange folder is a server path.
fn require_operator(state: &AppState, access: &RequestAccess) -> Result<(), AppError> {
    require_database_management_allowed(state)?;
    if access.role != AccessRole::ReadWrite {
        return Err(AppError::Forbidden(
            "Managing workers needs read-write access".into(),
        ));
    }
    Ok(())
}

/// Where task files go when no folder is chosen.
pub fn default_exchange(state: &AppState) -> std::path::PathBuf {
    state.storage_roots.cache.join("workers")
}

fn summary(state: &AppState) -> WorkersResponse {
    let (workers, queued) = state.workers.board.summary();
    let setting = state
        .registry_path
        .read()
        .unwrap()
        .as_deref()
        .and_then(|path| crate::db_registry::DbRegistry::load_or_init(path).ok())
        .and_then(|registry| registry.remote_workers)
        .and_then(|settings| settings.exchange_folder);
    WorkersResponse {
        tokens: state
            .workers
            .tokens()
            .into_iter()
            .map(|token| TokenSummary {
                id: token.id,
                name: token.name,
                created_at: token.created_at,
            })
            .collect(),
        workers,
        queued,
        exchange_folder: state.workers.board.exchange().display().to_string(),
        exchange_folder_setting: setting,
        default_exchange_folder: default_exchange(state).display().to_string(),
        exchange_subfolder: super::board::EXCHANGE_SUBFOLDER,
        placement: Placement {
            rc_astro: state.workers.board.placement(TaskKind::RcAstroStep),
            stack: Some(state.workers.board.placement(TaskKind::StackChannel)),
        },
    }
}

/// GET /api/workers
async fn list(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
) -> Result<Json<ApiResponse<WorkersResponse>>, AppError> {
    require_operator(&state, &access)?;
    let listed = Arc::clone(&state);
    let response = tokio::task::spawn_blocking(move || {
        // Lists this server's own worker, probing rc-astro once a minute.
        listed.workers.refresh_local();
        summary(&listed)
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))?;
    Ok(Json(ApiResponse::success(response)))
}

/// POST /api/workers/tokens
async fn create_token(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Json(request): Json<CreateTokenRequest>,
) -> Result<Json<ApiResponse<CreatedToken>>, AppError> {
    require_operator(&state, &access)?;
    if access.api_token {
        return Err(AppError::Forbidden(
            "Create worker tokens from a signed-in session, not with an API token".into(),
        ));
    }
    let (secret, record) = super::tokens::mint(&request.name).map_err(AppError::BadRequest)?;
    let stored = record.clone();
    let tokens = update_registry(&state, move |registry| {
        let settings = registry.remote_workers.get_or_insert_with(Default::default);
        if settings
            .tokens
            .iter()
            .any(|token| token.name.eq_ignore_ascii_case(&stored.name))
        {
            return Err(AppError::Conflict(format!(
                "A worker named {} already has a token; revoke it first, or choose another name",
                stored.name
            )));
        }
        settings.tokens.push(stored);
        Ok(settings.tokens.clone())
    })
    .await?;
    state.workers.set_tokens(tokens);
    tracing::info!(worker = record.name, "A worker token was created");
    Ok(Json(ApiResponse::success(CreatedToken {
        token: secret,
        summary: TokenSummary {
            id: record.id,
            name: record.name,
            created_at: record.created_at,
        },
        workers: summary(&state),
    })))
}

/// DELETE /api/workers/tokens/{id}: the worker using it is disconnected,
/// and its task goes back in the queue.
async fn revoke_token(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Path(id): Path<String>,
) -> Result<Json<ApiResponse<WorkersResponse>>, AppError> {
    require_operator(&state, &access)?;
    let tokens = update_registry(&state, |registry| {
        let settings = registry.remote_workers.get_or_insert_with(Default::default);
        let before = settings.tokens.len();
        settings.tokens.retain(|token| token.id != id);
        if settings.tokens.len() == before {
            return Err(AppError::NotFound);
        }
        Ok(settings.tokens.clone())
    })
    .await?;
    state.workers.set_tokens(tokens);
    Ok(Json(ApiResponse::success(summary(&state))))
}

/// PUT /api/workers/exchange
async fn set_exchange(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Json(request): Json<ExchangeRequest>,
) -> Result<Json<ApiResponse<WorkersResponse>>, AppError> {
    require_operator(&state, &access)?;
    let folder = request
        .folder
        .map(|folder| folder.trim().to_string())
        .filter(|folder| !folder.is_empty());
    let resolved = match &folder {
        Some(folder) => {
            let path = std::path::PathBuf::from(folder);
            if !path.is_absolute() {
                return Err(AppError::BadRequest(
                    "Enter the shared folder as an absolute path".into(),
                ));
            }
            path
        }
        None => default_exchange(&state),
    };
    let checked = resolved.clone();
    tokio::task::spawn_blocking(move || writable(&checked))
        .await
        .map_err(|error| AppError::InternalError(error.to_string()))?
        .map_err(AppError::BadRequest)?;
    update_registry(&state, |registry| {
        registry
            .remote_workers
            .get_or_insert_with(Default::default)
            .exchange_folder = folder;
        Ok(())
    })
    .await?;
    state.workers.board.set_exchange(resolved);
    Ok(Json(ApiResponse::success(summary(&state))))
}

/// PUT /api/workers/placement
async fn set_placement(
    State(state): State<Arc<AppState>>,
    Extension(access): Extension<RequestAccess>,
    Json(placement): Json<Placement>,
) -> Result<Json<ApiResponse<WorkersResponse>>, AppError> {
    require_operator(&state, &access)?;
    update_registry(&state, |registry| {
        let settings = registry.remote_workers.get_or_insert_with(Default::default);
        settings.rc_astro = placement.rc_astro;
        if let Some(stack) = placement.stack {
            settings.stack = stack;
        }
        Ok(())
    })
    .await?;
    let board = &state.workers.board;
    board.set_placement(TaskKind::RcAstroStep, placement.rc_astro);
    if let Some(stack) = placement.stack {
        board.set_placement(TaskKind::StackChannel, stack);
    }
    Ok(Json(ApiResponse::success(summary(&state))))
}

/// Check that PSF Guard can create `folder` and write in it.
fn writable(folder: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(folder)
        .map_err(|error| format!("{} could not be created: {error}", folder.display()))?;
    let probe = folder.join(format!(".psf-guard-write-check-{}", std::process::id()));
    std::fs::write(&probe, b"ok")
        .map_err(|error| format!("PSF Guard cannot write in {}: {error}", folder.display()))?;
    let _ = std::fs::remove_file(probe);
    Ok(())
}
