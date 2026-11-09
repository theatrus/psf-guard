//! A paired Director plugin's snapshot of its N.I.N.A. profile's star
//! detection settings. The same body and store as the Sync plugin's
//! (`/sync/v1/star-detection/profile`, see
//! [`crate::server::star_detection_settings`]); the catalog comes from the
//! rig's pairing rather than the body.
use super::*;
use crate::server::handlers::AppError;
use crate::server::star_detection_settings::{
    plugin_body, store_plugin_snapshot, PluginProfileRequest, PluginProfileResponse,
};
use crate::star_detection::RecordSource;
use axum::http::HeaderMap;

/// POST /api/director/v1/rigs/{rig}/star-detection-settings
pub(super) async fn submit(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
    body: Result<Json<PluginProfileRequest>, axum::extract::rejection::JsonRejection>,
) -> Result<Response, AppError> {
    let service = enabled(&state).map_err(|_| AppError::NotFound)?;
    // The pairing middleware checked the token, rig and profile header; read
    // the client again here for its catalog.
    let client = super::image_uploads::paired_client(&service, rig, &headers).await?;
    let request = plugin_body(body)?;
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
