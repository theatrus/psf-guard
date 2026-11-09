//! Image uploads from a paired Director plugin. The database comes from the
//! rig's pairing; the receive folder, layout, header import, calibration
//! matching and audit record are the Sync upload's
//! ([`crate::server::remote_upload`]), and the database's **Image uploads**
//! setting governs both plugins.
use super::*;
use crate::server::handlers::AppError;
use axum::extract::Multipart;
use axum::http::{header::AUTHORIZATION, HeaderMap};
use psf_guard_director_meta::client::Client;

/// The paired client behind this request, after the pairing middleware has
/// checked its token, rig and profile header.
pub(super) async fn paired_client(
    service: &Arc<Service>,
    rig: Uuid,
    headers: &HeaderMap,
) -> Result<Client, AppError> {
    let denied = || AppError::Forbidden("this rig's pairing does not cover the request".into());
    let secret = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
        .map(|(_, secret)| secret.trim())
        .filter(|secret| secret.starts_with("psfdrc_"))
        .ok_or_else(denied)?;
    let hash = crate::auth_registry::hash_token(secret);
    let client = service
        .clone()
        .query(move |store| store.client_for_token(&hash))
        .await
        .map_err(|_| AppError::InternalError("the Director store could not be read".into()))?
        .ok_or_else(denied)?;
    if client.rig_id != rig {
        return Err(denied());
    }
    Ok(client)
}

/// The registered database a paired client's catalog identity names.
fn paired_database(
    state: &AppState,
    service: &Service,
    client: &Client,
) -> Result<Arc<DatabaseContext>, AppError> {
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| AppError::InternalError("the database list could not be read".into()))?
        .values()
        .cloned()
        .collect();
    identified_catalogs(&catalogs, service.instance_id)
        .get(client.catalog_id)
        .cloned()
        .ok_or_else(|| {
            AppError::NotFoundMessage(
                "the database this rig is paired with is not registered".into(),
            )
        })
}

/// Whether the paired database takes image uploads now.
#[derive(Debug, Serialize)]
pub(super) struct UploadState {
    /// Uploads are on and the receive folder is usable.
    enabled: bool,
    /// Why uploads are off; absent when they are on.
    #[serde(skip_serializing_if = "Option::is_none")]
    reason: Option<&'static str>,
    /// The largest request the server accepts, in bytes.
    max_bytes: usize,
}

fn upload_state(database: &DatabaseContext) -> UploadState {
    let config = database.remote_image_upload.as_ref();
    let reason = if !config.is_some_and(|config| config.enabled) {
        Some("Image uploads are off for this database. Turn them on under Rig setup › Plugins.")
    } else if database.remote_image_upload_dir.is_none() {
        Some("The database's receive folder is not available on the server.")
    } else {
        None
    };
    UploadState {
        enabled: reason.is_none(),
        reason,
        max_bytes: crate::server::remote_upload::MAX_MULTIPART_BYTES,
    }
}

/// GET /api/director/v1/rigs/{rig}/uploads
pub(super) async fn state(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
) -> Result<Response, AppError> {
    let service = enabled(&state).map_err(|_| AppError::NotFound)?;
    let client = paired_client(&service, rig, &headers).await?;
    let database = paired_database(&state, &service, &client)?;
    Ok(super::pairing::no_store(Json(ApiResponse::success(
        upload_state(&database),
    ))))
}

/// POST /api/director/v1/rigs/{rig}/images
///
/// One multipart `image` field, with `X-PSF-Guard-Content-SHA256`, as the
/// Sync upload takes it.
pub(super) async fn upload(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<Response, AppError> {
    let service = enabled(&state).map_err(|_| AppError::NotFound)?;
    let client = paired_client(&service, rig, &headers).await?;
    let database = paired_database(&state, &service, &client)?;
    let answer =
        crate::server::remote_upload::receive_director_image(state, database, headers, multipart)
            .await?;
    Ok(super::pairing::no_store(answer))
}
