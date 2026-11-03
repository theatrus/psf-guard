//! A paired Director plugin's snapshot of its N.I.N.A. profile's star
//! detection settings. The same body and store as the Sync plugin's
//! (`/sync/v1/star-detection/profile`, see
//! [`crate::server::star_detection_settings`]); the catalog comes from the
//! rig's pairing rather than the body.
use super::*;
use crate::server::handlers::AppError;
use crate::server::star_detection_settings::{
    store_plugin_snapshot, PluginProfileRequest, PluginProfileResponse,
};
use crate::star_detection::RecordSource;
use axum::http::{header::AUTHORIZATION, HeaderMap};

/// POST /api/director/v1/rigs/{rig}/star-detection-settings
pub(super) async fn submit(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
    Json(request): Json<PluginProfileRequest>,
) -> Result<Response, AppError> {
    let denied = || AppError::Forbidden("this rig's pairing does not cover the request".into());
    let service = enabled(&state).map_err(|_| AppError::NotFound)?;
    // The pairing middleware checked the token, rig and profile header; read
    // the client again here for its catalog.
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
        .query(move |store| store.client_for_token(&hash))
        .await
        .map_err(|_| AppError::InternalError("the Director store could not be read".into()))?
        .ok_or_else(denied)?;
    if client.rig_id != rig {
        return Err(denied());
    }
    if request.catalog_id.is_some() {
        return Err(AppError::BadRequest(
            "a Director rig's catalog comes from its pairing; leave catalog_id out".into(),
        ));
    }
    let paired = Uuid::parse_str(request.profile_id.trim()).ok();
    if paired != Some(client.profile_id) {
        return Err(AppError::BadRequest(
            "profile_id is not the profile this rig is paired with".into(),
        ));
    }
    let response: PluginProfileResponse = store_plugin_snapshot(
        &state,
        RecordSource::DirectorPlugin,
        client.catalog_id.to_string(),
        request,
    )
    .await?;
    let mut response = Json(ApiResponse::success(response)).into_response();
    response.headers_mut().insert(
        axum::http::header::CACHE_CONTROL,
        axum::http::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
