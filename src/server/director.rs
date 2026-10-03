//! Opt-in coordinator management and scoped rig reporting, not acquisition authority.
//! The outer API middleware owns browser authentication and write-role checks.

use super::{api::ApiResponse, state::AppState};
use crate::server::database_context::DatabaseContext;
use axum::{
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{header::RETRY_AFTER, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use psf_guard_director_meta::{
    CatalogIdentity, Error as StoreError, IdentityPage, MetaStore, NamedIdentity, Uuid,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::{
    path::Path as FilePath,
    sync::{Arc, Mutex},
};
use tokio::sync::Semaphore;
mod activation;
mod allocation;
mod catalog_adoption;
mod catalog_discovery;
mod catalog_rig;
mod checkin;
mod equipment_report;
mod feasibility;
mod framing;
mod import_drafts;
mod mosaic;
pub(super) mod pairing;
mod plan;
mod plans;
mod program;
mod rig_profile;
mod sky_image;
mod sky_objects;
mod sky_search;
mod templates;
mod workload;

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
/// The meta store sits beside the registry unless a path is given. Planning
/// runs on every server; only writes into rig databases need management.
pub(crate) fn resolve_meta_path(
    explicit: Option<&FilePath>,
    registry: Option<&FilePath>,
) -> Option<std::path::PathBuf> {
    match explicit {
        Some(path) => Some(path.to_path_buf()),
        None => registry.map(default_meta_path),
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

/// How many reads of the meta store run at once. SQLite in WAL mode serves
/// any number of readers beside one writer; this only bounds the blocking
/// threads a burst of requests can take.
fn reader_slots() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .clamp(2, 8)
}

/// The meta store and the two gates on it. Reads share a pool of read-only
/// connections and never queue behind a write; writes take the one writer in
/// turn. `discovery_admission` is the separate gate on work that scans or
/// writes rig databases, since that takes seconds and must not stall the
/// store.
pub struct Service {
    path: std::path::PathBuf,
    writer: Mutex<MetaStore>,
    readers: Mutex<Vec<MetaStore>>,
    read_slots: Arc<Semaphore>,
    instance_id: Uuid,
    admission: Arc<Semaphore>,
    discovery_admission: Arc<Semaphore>,
}

impl Service {
    /// Call during startup, never with a client-supplied filesystem path.
    pub(crate) fn configured(path: Option<&FilePath>) -> anyhow::Result<Option<Arc<Self>>> {
        let Some(path) = path else { return Ok(None) };
        let store = if path.try_exists()? {
            MetaStore::open(path)?
        } else {
            MetaStore::create(path)?
        };
        Ok(Some(Arc::new(Self {
            path: path.to_path_buf(),
            instance_id: store.instance_id(),
            writer: Mutex::new(store),
            readers: Mutex::new(Vec::new()),
            read_slots: Arc::new(Semaphore::new(reader_slots())),
            admission: Arc::new(Semaphore::new(1)),
            discovery_admission: Arc::new(Semaphore::new(1)),
        })))
    }

    /// A write, in turn behind every other write. Dropping the HTTP request
    /// must not release the permit while its SQLite write still runs, so the
    /// permit travels into the blocking task.
    async fn run<T: Send + 'static>(
        self: Arc<Self>,
        operation: impl FnOnce(&mut MetaStore) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        self.with_writer(move |store| operation(store).map_err(Error::from))
            .await
    }

    /// A read on a pooled read-only connection: it runs beside any write
    /// and beside other reads, and sees the last committed state.
    async fn query<T: Send + 'static>(
        self: Arc<Self>,
        operation: impl FnOnce(&MetaStore) -> Result<T, StoreError> + Send + 'static,
    ) -> Result<T, Error> {
        self.with_reader(move |store| operation(store).map_err(Error::from))
            .await
    }

    /// Like [`Self::run`] for a handler with its own error type; the
    /// closure may do other blocking work (rig databases) around the store.
    async fn with_writer<T, E>(
        self: Arc<Self>,
        operation: impl FnOnce(&mut MetaStore) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<Error> + Send + 'static,
    {
        let permit = admit(&self.admission).await?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let mut store = self.writer.lock().map_err(|_| Error::Internal)?;
            operation(&mut store)
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, "Director metadata worker failed");
            Error::Internal
        })?
    }

    /// Like [`Self::query`] for a handler with its own error type.
    async fn with_reader<T, E>(
        self: Arc<Self>,
        operation: impl FnOnce(&MetaStore) -> Result<T, E> + Send + 'static,
    ) -> Result<T, E>
    where
        T: Send + 'static,
        E: From<Error> + Send + 'static,
    {
        let slot = admit(&self.read_slots).await?;
        tokio::task::spawn_blocking(move || {
            let _slot = slot;
            let reader = self.readers.lock().map_err(|_| Error::Internal)?.pop();
            let reader = match reader {
                Some(reader) => reader,
                None => MetaStore::open_reader(&self.path).map_err(Error::from)?,
            };
            let result = operation(&reader);
            if let Ok(mut idle) = self.readers.lock() {
                idle.push(reader);
            }
            result
        })
        .await
        .map_err(|error| {
            tracing::error!(%error, "Director metadata reader failed");
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

/// Every registered database that carries a catalog identity, one per
/// identity. Two files with the same identity are one catalog copied by hand;
/// the first by slug stands for it and the rest are named, never merged, so a
/// copy can not receive a plan meant for the original.
pub(super) struct IdentifiedCatalogs {
    by_id: BTreeMap<Uuid, (CatalogIdentity, Arc<DatabaseContext>)>,
    /// One line per file left out, for the operator.
    pub(super) duplicates: Vec<String>,
    /// Slugs of the files left out.
    pub(super) duplicate_slugs: Vec<String>,
}

impl IdentifiedCatalogs {
    pub(super) fn get(&self, id: Uuid) -> Option<&Arc<DatabaseContext>> {
        self.by_id.get(&id).map(|(_, context)| context)
    }

    pub(super) fn iter(&self) -> impl Iterator<Item = (&CatalogIdentity, &Arc<DatabaseContext>)> {
        self.by_id
            .values()
            .map(|(identity, context)| (identity, context))
    }

    pub(super) fn is_duplicate(&self, slug: &str) -> bool {
        self.duplicate_slugs.iter().any(|s| s == slug)
    }
}

/// The identity an unadopted file gets until one is written into it: fixed
/// by this instance and the file's path, so a read-only server sees the same
/// rig across restarts and the same id is written once management allows.
pub(super) fn derived_identity(instance: Uuid, path: &str) -> CatalogIdentity {
    let canonical = dunce::canonicalize(path)
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| path.to_owned());
    CatalogIdentity {
        id: Uuid::new_v5(&instance, canonical.as_bytes()),
        origin_instance_id: instance,
    }
}

/// The identity a file carries, or the one it would be given.
pub(super) fn identity_of(instance: Uuid, path: &str) -> CatalogIdentity {
    read_identity(path).unwrap_or_else(|| derived_identity(instance, path))
}

/// Read each file's identity once, in slug order so the choice is stable. An
/// unadopted file counts under its derived identity, so a read-only server
/// still plans over it.
pub(super) fn identified_catalogs(
    catalogs: &[Arc<DatabaseContext>],
    instance: Uuid,
) -> IdentifiedCatalogs {
    let mut sorted: Vec<&Arc<DatabaseContext>> = catalogs.iter().collect();
    sorted.sort_by(|a, b| a.id.cmp(&b.id));
    let mut found = IdentifiedCatalogs {
        by_id: BTreeMap::new(),
        duplicates: Vec::new(),
        duplicate_slugs: Vec::new(),
    };
    for context in sorted {
        let identity = identity_of(instance, &context.database_path);
        match found.by_id.get(&identity.id) {
            Some((_, first)) => {
                found.duplicates.push(format!(
                    "{}: carries the same catalog identity as {}, so it is a copy of that file; it is left out of planning. Remove it from the registry, or drop its psf_guard_catalog_identity table to make it a database of its own.",
                    context.name, first.name
                ));
                found.duplicate_slugs.push(context.id.clone());
            }
            None => {
                found.by_id.insert(identity.id, (identity, context.clone()));
            }
        }
    }
    found
}

/// The identity a database file carries, or `None` for an unadopted or
/// unreadable file. Never writes.
pub(super) fn read_identity(path: &str) -> Option<CatalogIdentity> {
    let connection = super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(path),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    connection
        .busy_timeout(std::time::Duration::from_secs(1))
        .ok()?;
    crate::catalog_identity::read(&connection).ok().flatten()
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .merge(pairing::routes())
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
            "/rigs/{rig}/equipment-reports",
            get(equipment_report::list)
                .post(equipment_report::submit)
                .layer(DefaultBodyLimit::max(
                    psf_guard_director_core::MAX_REQUEST_BYTES,
                )),
        )
        .route(
            "/rigs/{rig}/equipment-reports/{client}/accept",
            axum::routing::post(equipment_report::accept),
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
            "/rigs/{rig}/allocation",
            get(allocation::get_allocation).post(allocation::admit),
        )
        .route(
            "/rigs/{rig}/allocation/start",
            axum::routing::post(allocation::start),
        )
        .route(
            "/rigs/{rig}/workload-policy",
            get(workload::get_policy).put(workload::commission),
        )
        .route(
            "/rigs/{rig}/workloads/request",
            axum::routing::post(workload::request),
        )
        .route(
            "/rigs/{rig}/workloads/release",
            axum::routing::post(workload::release),
        )
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
        .route("/plans", get(plans::list))
        .route(
            "/projects/{id}/feasibility",
            axum::routing::post(feasibility::evaluate),
        )
        .route("/projects/{id}/activation", get(activation::last))
        .route("/projects/{id}/mosaic", get(mosaic::get))
        .route(
            "/projects/{id}/activation/preview",
            axum::routing::post(activation::preview),
        )
        .route(
            "/projects/{id}/activation/apply",
            axum::routing::post(activation::apply),
        )
        .route(
            "/projects/{id}/activation/push",
            axum::routing::post(activation::push),
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
        .route("/sky/resolve", get(sky_image::resolve))
        .route("/sky/search", get(sky_search::search))
        .route("/sky/objects", get(sky_objects::marks))
        .route("/projects/{id}", get(project).patch(rename_project))
        .route("/projects/{id}/attach", axum::routing::post(attach_project))
        .route("/projects/{id}/detach", axum::routing::post(detach_project))
        .merge(configuration_api::routes())
        .merge(preferences::routes())
        .merge(templates::routes())
        .layer(DefaultBodyLimit::max(4096))
}

/// How long a request waits for its turn at a gate before it answers 503.
/// Writes queue behind one another and adoption of every database can take
/// seconds, so the wait must cover a queue of them; the browser retries a
/// 503 anyway. Reads do not wait on writes at all.
const ADMISSION_WAIT: std::time::Duration = if cfg!(test) {
    // Tests hold the gate on purpose to see the busy answer; they should not
    // sit through the production wait for it.
    std::time::Duration::from_secs(1)
} else {
    std::time::Duration::from_secs(20)
};

async fn admit(semaphore: &Arc<Semaphore>) -> Result<tokio::sync::OwnedSemaphorePermit, Error> {
    tokio::time::timeout(ADMISSION_WAIT, semaphore.clone().acquire_owned())
        .await
        .map_err(|_| Error::Busy)?
        .map_err(|_| Error::Internal)
}

/// Planning reads and the meta store are open to every server.
fn enabled(state: &AppState) -> Result<Arc<Service>, Error> {
    state.director.clone().ok_or(Error::Disabled)
}

/// Writes into rig databases (activation, adoption tables, pushes) need the
/// server's database-management permission on top.
fn writable(state: &AppState) -> Result<Arc<Service>, Error> {
    let service = enabled(state)?;
    if !state.database_management_allowed() {
        return Err(Error::Forbidden);
    }
    Ok(service)
}

#[derive(Serialize)]
struct Status {
    protocol_version: u32,
    enabled: bool,
    instance_id: Option<Uuid>,
    acquisition_available: bool,
    /// Whether this server may write into rig databases: activate plans,
    /// adopt catalogs in place, and edit Target Scheduler rows. Without it,
    /// Planning is read-only over the catalogs.
    database_management: bool,
}

async fn status(State(state): State<Arc<AppState>>) -> Json<ApiResponse<Status>> {
    let service = enabled(&state).ok();
    Json(ApiResponse::success(Status {
        protocol_version: 1,
        enabled: service.is_some(),
        instance_id: service.map(|s| s.instance_id),
        acquisition_available: false,
        database_management: state.database_management_allowed(),
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
            .query(move |store| store.projects(page.after, page.limit))
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
        .query(move |store| store.project(id))
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AttachRequest {
    /// The plan to absorb: every database link it has moves to this plan.
    from_project_id: Uuid,
}

/// Two databases that each made their own project for one target are two
/// plans until one is attached to the other. Only the meta store moves; the
/// next activation takes the attached project's rows over where they stand.
async fn attach_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<AttachRequest>,
) -> Result<Json<ApiResponse<psf_guard_director_meta::merge::Attached>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| store.attach_project(id, request.from_project_id))
            .await?,
    )))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DetachRequest {
    catalog_slug: String,
    source_project_guid: Uuid,
    /// The name for the plan the project gets back; its own name, as a rule.
    name: String,
}

/// Give one database's project a plan of its own again.
async fn detach_project(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(request): Json<DetachRequest>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    let service = enabled(&state)?;
    let catalog = state
        .get_database(&request.catalog_slug)
        .ok_or(Error::Missing)?;
    let identity = identity_of(service.instance_id, &catalog.database_path);
    let name = request.name.trim().to_owned();
    if name.is_empty() || name.len() > 256 {
        return Err(Error::Invalid);
    }
    Ok(Json(ApiResponse::success(
        service
            .run(move |store| {
                store.detach_project(
                    id,
                    identity.id,
                    request.source_project_guid,
                    Uuid::new_v4(),
                    &name,
                )
            })
            .await?,
    )))
}

mod configuration_api;
mod preferences;
#[cfg(test)]
mod tests;
