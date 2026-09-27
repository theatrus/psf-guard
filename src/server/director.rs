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
mod activation;
mod catalog_adoption;
mod catalog_discovery;
mod catalog_rig;
mod checkin;
mod framing;
mod plan;
mod program;
mod rig_profile;
mod sky_image;

/// The default store sits beside the registry, like `auth.json`, so a test
/// registry gets its own meta store and nothing lands in the real config dir.
pub(crate) fn default_meta_path(registry: &FilePath) -> std::path::PathBuf {
    if registry.file_name().and_then(|name| name.to_str()) == Some("config.json") {
        return registry.with_file_name("director-meta.sqlite");
    }
    let stem = registry
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or("config");
    registry.with_file_name(format!("{stem}.director-meta.sqlite"))
}

/// An explicit path always wins. Without one, a server that may manage its
/// databases opens the default store; a read-only server leaves Director off,
/// because planning writes into rig databases at activation.
pub(crate) fn resolve_meta_path(
    explicit: Option<&FilePath>,
    registry: Option<&FilePath>,
    management: bool,
) -> Option<std::path::PathBuf> {
    match explicit {
        Some(path) => Some(path.to_path_buf()),
        None if management => registry.map(default_meta_path),
        None => None,
    }
}

pub(super) fn validate_registry_separation(
    meta: Option<&FilePath>,
    registry: Option<&FilePath>,
) -> anyhow::Result<()> {
    let (Some(meta), Some(registry)) = (meta, registry) else {
        return Ok(());
    };
    fn location(path: &FilePath) -> anyhow::Result<Option<std::path::PathBuf>> {
        if path.try_exists()? {
            return Ok(Some(dunce::canonicalize(path)?));
        }
        let absolute = std::path::absolute(path)?;
        let parent = absolute
            .parent()
            .ok_or_else(|| anyhow::anyhow!("Invalid metadata path"))?;
        let parent = match dunce::canonicalize(parent) {
            Ok(parent) => parent,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        Ok(absolute.file_name().map(|name| parent.join(name)))
    }
    let meta = location(meta)?;
    let protected = [
        registry.to_path_buf(),
        crate::auth_registry::AuthRegistry::path_for_database_registry(registry),
        crate::processing_setups::ProcessingSetupsRegistry::path_for_database_registry(registry),
    ];
    for path in protected {
        if let (Some(meta), Some(other)) = (&meta, location(&path)?) {
            #[cfg(windows)]
            let same = meta
                .to_string_lossy()
                .eq_ignore_ascii_case(&other.to_string_lossy());
            #[cfg(not(windows))]
            let same = meta == &other;
            anyhow::ensure!(
                !same,
                "Director metadata must be separate from server registry files"
            );
        }
    }
    Ok(())
}

pub struct Service {
    store: Mutex<MetaStore>,
    instance_id: Uuid,
    admission: Arc<Semaphore>,
    discovery_admission: Arc<Semaphore>,
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
            discovery_admission: Arc::new(Semaphore::new(1)),
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
    /// The coordinator, catalog and rig identities in a report do not agree.
    WrongRig,
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

impl From<crate::catalog_identity::Error> for Error {
    fn from(error: crate::catalog_identity::Error) -> Self {
        use crate::catalog_identity::Error as IdentityError;
        match error {
            IdentityError::Sqlite(error) => StoreError::from(error).into(),
            IdentityError::Conflict => Self::Conflict,
            IdentityError::InvalidIdentity => Self::Invalid,
            other => {
                tracing::error!(error = ?other, "Director catalog identity failed");
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
            Self::Missing => (StatusCode::NOT_FOUND, "Director record not found"),
            Self::Conflict => (
                StatusCode::CONFLICT,
                "Director record conflicts with stored content; reload before retrying",
            ),
            Self::WrongRig => (
                StatusCode::FORBIDDEN,
                "Coordinator, catalog and rig identities do not match this server's binding",
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
        .route(
            "/catalogs/{slug}/discovery",
            get(catalog_discovery::discover),
        )
        .route("/projects", get(list_projects).post(create_project))
        .route("/catalogs/{slug}/mappings", get(catalog_adoption::mappings))
        .route(
            "/catalogs/{slug}/rig/preview",
            axum::routing::post(catalog_rig::preview),
        )
        .route(
            "/catalogs/{slug}/rig/apply",
            axum::routing::post(catalog_rig::apply),
        )
        .route(
            "/catalogs/{slug}/rig/profile",
            get(rig_profile::get)
                .put(rig_profile::put)
                .layer(DefaultBodyLimit::max(
                    psf_guard_director_core::MAX_REQUEST_BYTES,
                )),
        )
        .route(
            "/rigs/{rig}/equipment",
            axum::routing::put(rig_profile::report_equipment).layer(DefaultBodyLimit::max(
                psf_guard_director_core::MAX_REQUEST_BYTES,
            )),
        )
        .route(
            "/catalogs/{slug}/adoption/preview",
            axum::routing::post(catalog_adoption::preview).layer(DefaultBodyLimit::max(
                psf_guard_director_core::MAX_REQUEST_BYTES,
            )),
        )
        .route(
            "/catalogs/{slug}/adoption/apply",
            axum::routing::post(catalog_adoption::apply).layer(DefaultBodyLimit::max(
                psf_guard_director_core::MAX_REQUEST_BYTES,
            )),
        )
        .route(
            "/framing/preview",
            axum::routing::post(framing::preview).layer(DefaultBodyLimit::max(
                psf_guard_director_core::MAX_REQUEST_BYTES,
            )),
        )
        .route(
            "/projects/{id}/framing",
            get(framing::get_draft)
                .put(framing::put_draft)
                .layer(DefaultBodyLimit::max(
                    psf_guard_director_core::MAX_REQUEST_BYTES,
                )),
        )
        .route("/rigs/profiles", get(framing::rig_profiles))
        .route("/catalogs/{slug}/templates", get(plan::templates))
        .route("/rigs/{rig}/program", get(program::pull))
        .route(
            "/rigs/{rig}/checkin",
            axum::routing::post(checkin::check_in).layer(DefaultBodyLimit::max(
                psf_guard_director_core::MAX_REQUEST_BYTES * 4,
            )),
        )
        .route(
            "/rigs/{rig}/status",
            axum::routing::post(checkin::report_status).layer(DefaultBodyLimit::max(
                psf_guard_director_core::MAX_REQUEST_BYTES,
            )),
        )
        .route("/rigs/status", get(checkin::statuses))
        .route("/projects/{id}/activation", get(activation::last))
        .route(
            "/projects/{id}/activation/preview",
            axum::routing::post(activation::preview),
        )
        .route(
            "/projects/{id}/activation/apply",
            axum::routing::post(activation::apply),
        )
        .route(
            "/projects/{id}/plan",
            get(plan::get_plan)
                .put(plan::put_plan)
                .layer(DefaultBodyLimit::max(
                    psf_guard_director_core::MAX_REQUEST_BYTES,
                )),
        )
        .route("/sky/surveys", get(sky_image::surveys))
        .route("/sky/cutout", get(sky_image::cutout))
        .route("/projects/{id}", get(project).patch(rename_project))
        .merge(configuration_api::routes())
        .layer(DefaultBodyLimit::max(4096))
}

/// Wait briefly for an admission permit instead of failing at once. Reads
/// that merely list state should not lose to a neighbouring request; a write
/// still uses `try_acquire_owned` so a dropped request cannot queue work.
async fn admit(semaphore: &Arc<Semaphore>) -> Result<tokio::sync::OwnedSemaphorePermit, Error> {
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        semaphore.clone().acquire_owned(),
    )
    .await
    .map_err(|_| Error::Busy)?
    .map_err(|_| Error::Internal)
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
struct CreateIdentity {
    id: Uuid,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameIdentity {
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
    Json(request): Json<CreateIdentity>,
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
    Json(request): Json<RenameIdentity>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| store.rename_project(id, request.expected_revision, &request.name))
            .await?,
    )))
}

mod configuration_api;
#[cfg(test)]
mod tests;
