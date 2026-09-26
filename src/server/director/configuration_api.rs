use super::*;
use axum::Extension;
use psf_guard_director_core::MAX_REQUEST_BYTES;
use psf_guard_director_meta::configuration::{RigSetup, SiteSnapshot, SnapshotIds};

#[derive(Clone, Copy)]
enum Identity {
    Site,
    Rig,
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    let mut router = Router::new();
    for (path, kind) in [("/sites", Identity::Site), ("/rigs", Identity::Rig)] {
        router = router
            .route(path, get(list).post(create).layer(Extension(kind)))
            .route(
                &format!("{path}/{{id}}"),
                get(identity).patch(rename).layer(Extension(kind)),
            );
    }
    router
        .route(
            "/sites/{site}/snapshots",
            get(site_ids)
                .post(register_site)
                .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
        .route("/sites/{site}/snapshots/{snapshot}", get(site_snapshot))
        .route(
            "/rigs/{rig}/setups",
            get(setup_ids)
                .post(register_setup)
                .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)),
        )
        .route("/rigs/{rig}/setups/{setup}", get(rig_setup))
}

async fn list(
    State(state): State<Arc<AppState>>,
    Extension(kind): Extension<Identity>,
    Query(page): Query<Page>,
) -> Result<Json<ApiResponse<IdentityPage>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| match kind {
                Identity::Site => store.sites(page.after, page.limit),
                Identity::Rig => store.rigs(page.after, page.limit),
            })
            .await?,
    )))
}

async fn create(
    State(state): State<Arc<AppState>>,
    Extension(kind): Extension<Identity>,
    Json(request): Json<CreateIdentity>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| match kind {
                Identity::Site => store.create_site(request.id, &request.name),
                Identity::Rig => store.create_rig(request.id, &request.name),
            })
            .await?,
    )))
}

async fn identity(
    State(state): State<Arc<AppState>>,
    Extension(kind): Extension<Identity>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    let value = enabled(&state)?
        .run(move |store| match kind {
            Identity::Site => store.site(id),
            Identity::Rig => store.rig(id),
        })
        .await?
        .ok_or(Error::Missing)?;
    Ok(Json(ApiResponse::success(value)))
}

async fn rename(
    State(state): State<Arc<AppState>>,
    Extension(kind): Extension<Identity>,
    Path(id): Path<Uuid>,
    Json(request): Json<RenameIdentity>,
) -> Result<Json<ApiResponse<NamedIdentity>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| match kind {
                Identity::Site => store.rename_site(id, request.expected_revision, &request.name),
                Identity::Rig => store.rename_rig(id, request.expected_revision, &request.name),
            })
            .await?,
    )))
}

async fn site_ids(
    State(state): State<Arc<AppState>>,
    Path(site): Path<Uuid>,
    Query(page): Query<Page>,
) -> Result<Json<ApiResponse<SnapshotIds>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| {
                store.site(site)?.ok_or(StoreError::NotFound)?;
                store.site_snapshot_ids(site, page.after, page.limit)
            })
            .await?,
    )))
}

async fn setup_ids(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Query(page): Query<Page>,
) -> Result<Json<ApiResponse<SnapshotIds>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| {
                store.rig(rig)?.ok_or(StoreError::NotFound)?;
                store.rig_setup_ids(rig, page.after, page.limit)
            })
            .await?,
    )))
}

async fn register_site(
    State(state): State<Arc<AppState>>,
    Path(site): Path<Uuid>,
    Json(snapshot): Json<SiteSnapshot>,
) -> Result<Json<ApiResponse<SiteSnapshot>>, Error> {
    if snapshot.site_id != site {
        return Err(Error::Invalid);
    }
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| {
                store.register_site_snapshot(&snapshot)?;
                Ok(snapshot)
            })
            .await?,
    )))
}

async fn register_setup(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Json(setup): Json<RigSetup>,
) -> Result<Json<ApiResponse<RigSetup>>, Error> {
    if setup.configuration.rig_id != rig.to_string() {
        return Err(Error::Invalid);
    }
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .run(move |store| {
                store.register_rig_setup(&setup)?;
                Ok(setup)
            })
            .await?,
    )))
}

async fn site_snapshot(
    State(state): State<Arc<AppState>>,
    Path((site, snapshot)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<SiteSnapshot>>, Error> {
    let value = enabled(&state)?
        .run(move |store| store.site_snapshot(snapshot))
        .await?
        .filter(|value| value.site_id == site)
        .ok_or(Error::Missing)?;
    Ok(Json(ApiResponse::success(value)))
}

async fn rig_setup(
    State(state): State<Arc<AppState>>,
    Path((rig, setup)): Path<(Uuid, Uuid)>,
) -> Result<Json<ApiResponse<RigSetup>>, Error> {
    let value = enabled(&state)?
        .run(move |store| store.rig_setup(setup))
        .await?
        .filter(|value| value.configuration.rig_id == rig.to_string())
        .ok_or(Error::Missing)?;
    Ok(Json(ApiResponse::success(value)))
}
