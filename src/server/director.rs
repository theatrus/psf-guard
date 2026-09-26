//! Opt-in coordinator management, not a rig pairing or acquisition protocol.
//! The outer API middleware owns browser authentication and write-role checks.

use super::{api::ApiResponse, state::AppState};
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header::RETRY_AFTER, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use psf_guard_director_meta::{Error as StoreError, IdentityPage, MetaStore, NamedIdentity, Uuid};
use serde::{Deserialize, Serialize};
use std::{
    path::Path as FilePath,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;

pub struct Service {
    store: Mutex<MetaStore>,
    instance_id: Uuid,
    admission: Arc<Semaphore>,
}

impl Service {
    /// Call during startup, never with a client-supplied filesystem path.
    pub(crate) fn configured(
        path: Option<&FilePath>,
        management: bool,
    ) -> anyhow::Result<Option<Arc<Self>>> {
        let Some(path) = path else { return Ok(None) };
        anyhow::ensure!(
            management,
            "Director metadata requires database management to be enabled"
        );
        let store = if path.try_exists()? {
            MetaStore::open(path)?
        } else {
            MetaStore::create(path)?
        };
        Ok(Some(Arc::new(Self {
            instance_id: store.instance_id(),
            store: Mutex::new(store),
            admission: Arc::new(Semaphore::new(1)),
        })))
    }

    async fn run<T: Send + 'static>(
        self: Arc<Self>,
        operation: impl FnOnce(&mut MetaStore) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        // Bound admission before scheduling blocking work. Dropping an HTTP
        // request must not release the permit while its SQLite write still runs.
        let permit = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| Error::Busy)?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut store = self.store.lock().map_err(|_| Error::Internal)?;
            operation(&mut store).map_err(Error::from)
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, "Director metadata worker failed");
            Error::Internal
        })?
    }
}

#[derive(Debug)]
enum Error {
    Disabled,
    Forbidden,
    Invalid,
    Missing,
    Conflict,
    Busy,
    Internal,
}

impl From<StoreError> for Error {
    fn from(error: StoreError) -> Self {
        match error {
            StoreError::InvalidInput => Self::Invalid,
            StoreError::NotFound => Self::Missing,
            StoreError::Conflict => Self::Conflict,
            StoreError::Sqlite(rusqlite::Error::SqliteFailure(code, _))
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Self::Busy
            }
            other => {
                tracing::error!(error = ?other, "Director metadata storage failed");
                Self::Internal
            }
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Disabled => (StatusCode::NOT_FOUND, "Director metadata is disabled"),
            Self::Forbidden => (
                StatusCode::FORBIDDEN,
                "Director metadata requires database management",
            ),
            Self::Invalid => (StatusCode::BAD_REQUEST, "Invalid Director metadata request"),
            Self::Missing => (StatusCode::NOT_FOUND, "Director project not found"),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "Director project changed; reload before retrying",
            ),
            Self::Busy => (
                StatusCode::SERVICE_UNAVAILABLE,
                "Director metadata is busy; retry shortly",
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Director metadata operation failed; see server logs",
            ),
        };
        let mut response = (status, Json(ApiResponse::<()>::error(message.into()))).into_response();
        if status == StatusCode::SERVICE_UNAVAILABLE {
            response
                .headers_mut()
                .insert(RETRY_AFTER, "1".parse().unwrap());
        }
        response
    }
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/status", get(status))
        .route("/projects", get(list_projects).post(create_project))
        .route("/projects/{id}", get(project).patch(rename_project))
        .layer(DefaultBodyLimit::max(4096))
}

fn enabled(state: &AppState) -> Result<Arc<Service>, Error> {
    if !state.database_management_allowed() {
        return Err(Error::Forbidden);
    }
    state.director.clone().ok_or(Error::Disabled)
}

#[derive(Serialize)]
struct Status {
    protocol_version: u32,
    enabled: bool,
    instance_id: Option<Uuid>,
    acquisition_available: bool,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<ApiResponse<Status>> {
    let service = enabled(&state).ok();
    Json(ApiResponse::success(Status {
        protocol_version: 1,
        enabled: service.is_some(),
        instance_id: service.map(|s| s.instance_id),
        acquisition_available: false,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Page {
    after: Option<Uuid>,
    #[serde(default = "page_size")]
    limit: usize,
}
fn page_size() -> usize {
    64
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateProject {
    id: Uuid,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameProject {
    expected_revision: u64,
    name: String,
}

async fn list_projects(
    State(state): State<Arc<AppState>>,
    Query(page): Query<Page>,
) -> Result<Json<ApiResponse<IdentityPage>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| store.projects(page.after, page.limit))
            .await?,
    )))
}

async fn create_project(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateProject>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| store.create_project(request.id, &request.name))
            .await?,
    )))
}

async fn project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    let record = enabled(&state)?
        .run(move |store| store.project(id))
        .await?
        .ok_or(Error::Missing)?;
    Ok(Json(ApiResponse::success(record)))
}

async fn rename_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<RenameProject>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| store.rename_project(id, request.expected_revision, &request.name))
            .await?,
    )))
}

#[cfg(test)]
mod tests;
