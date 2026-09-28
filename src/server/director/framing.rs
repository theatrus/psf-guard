//! Framing geometry and drafts for the planning view. The shared core does
//! the arithmetic; the browser draws it. A draft is editable intent, not an
//! allocation, and the preview is stateless.

use super::*;
use psf_guard_director_core::{
    bandpass::{default_exposure_seconds, BandpassKind, ExposureContext},
    framing::{FramingPreview, FramingRequest},
    optics::FieldOfView,
};
use psf_guard_director_meta::{framing::FramingDraft, profile::RigProfile};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Json(request): Json<FramingRequest>,
) -> Result<Json<ApiResponse<FramingPreview>>, Error> {
    enabled(&state)?;
    let preview = request.preview().map_err(|_| Error::Invalid)?;
    Ok(Json(ApiResponse::success(preview)))
}

#[derive(Serialize)]
pub(super) struct DraftView {
    project: NamedIdentity,
    draft: Option<FramingDraft>,
}

pub(super) async fn get_draft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<DraftView>>, Error> {
    let view = enabled(&state)?
        .run(move |store| {
            let project = store.project(id)?.ok_or(StoreError::NotFound)?;
            let draft = store.framing_draft(id)?;
            Ok(DraftView { project, draft })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}

/// The body is the whole draft; its `revision` is the one the caller read.
pub(super) async fn put_draft(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(mut draft): Json<FramingDraft>,
) -> Result<Json<ApiResponse<DraftView>>, Error> {
    if draft.project_id != id {
        return Err(Error::Invalid);
    }
    draft.updated_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let view = enabled(&state)?
        .run(move |store| {
            let project = store.project(id)?.ok_or(StoreError::NotFound)?;
            let expected = draft.revision;
            let saved = store.save_framing_draft(&draft, expected)?;
            Ok(DraftView {
                project,
                draft: Some(saved),
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}

#[derive(Serialize)]
pub(super) struct RigProfileSummary {
    rig: NamedIdentity,
    catalog_slug: String,
    catalog_name: String,
    profile: Option<RigProfile>,
    field_of_view: Option<FieldOfView>,
    /// Starting exposure lengths for this rig's optics and sky, per band kind.
    default_exposure_seconds: DefaultExposures,
}

#[derive(Serialize)]
pub(super) struct DefaultExposures {
    broadband: f64,
    narrowband: f64,
}

fn default_exposures(
    profile: Option<&RigProfile>,
    field_of_view: Option<&FieldOfView>,
) -> DefaultExposures {
    let context = |kind| ExposureContext {
        focal_ratio: field_of_view.and_then(|fov| fov.focal_ratio),
        bortle_class: profile
            .and_then(|p| p.sky_quality.as_ref())
            .map(|sky| sky.value.bortle_class),
        kind,
    };
    DefaultExposures {
        broadband: default_exposure_seconds(context(BandpassKind::Broadband)),
        narrowband: default_exposure_seconds(context(BandpassKind::Narrowband)),
    }
}

/// Every registered database that is bound to a rig, with the rig's profile.
/// Databases that cannot be opened or are not bound are left out, not failed.
pub(super) async fn rig_profiles(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<RigProfileSummary>>>, Error> {
    let service = enabled(&state)?;
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let metadata_permit = admit(&service.admission).await?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let summaries = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let store = service.store.lock().map_err(|_| Error::Internal)?;
        let mut summaries = Vec::new();
        for (identity, catalog) in identified_catalogs(&catalogs, service.instance_id).iter() {
            let Some(binding) = store.catalog_rig(identity.id)? else {
                continue;
            };
            let profile = store.rig_profile(binding.rig.id)?;
            let field_of_view = profile
                .as_ref()
                .and_then(|p| p.optics.as_ref())
                .and_then(|optics| optics.value.field_of_view().ok());
            summaries.push(RigProfileSummary {
                rig: binding.rig,
                catalog_slug: catalog.id.clone(),
                catalog_name: catalog.name.clone(),
                default_exposure_seconds: default_exposures(
                    profile.as_ref(),
                    field_of_view.as_ref(),
                ),
                profile,
                field_of_view,
            });
        }
        summaries.sort_by(|a, b| a.catalog_name.cmp(&b.catalog_name));
        Ok::<_, Error>(summaries)
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director rig profile listing failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(summaries)))
}
