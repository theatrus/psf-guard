//! Explicit first-allocation admission. Preview GETs never write this record.
use super::*;
use crate::server::auth::RequestAccess;
use axum::{
    http::{header::CACHE_CONTROL, HeaderValue},
    Extension,
};
use psf_guard_director_meta::allocation::Allocation;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Scope {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Admission {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    allocation_id: Uuid,
    client_id: Uuid,
    preview_revision: String,
}

fn response(result: Result<Allocation, program::PullError>) -> Response {
    let mut response = match result {
        Ok(value) => Json(ApiResponse::success(value)).into_response(),
        Err(error) => error.into_response(),
    };
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) async fn get_allocation(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Query(scope): Query<Scope>,
) -> Response {
    response(
        async {
            let service = enabled(&state)?;
            if scope.coordinator_instance_id != service.instance_id {
                return Err(Error::WrongRig.into());
            }
            let allocation = service
                .query(move |store| store.allocation(rig))
                .await?
                .ok_or(Error::Missing)?;
            if allocation.catalog_id != scope.catalog_id {
                return Err(Error::WrongRig.into());
            }
            Ok(allocation)
        }
        .await,
    )
}

pub(super) async fn admit(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    Json(input): Json<Admission>,
) -> Response {
    if let Err(response) = pairing::operator(&access) {
        return *response;
    }
    response(
        async {
            let service = enabled(&state)?;
            if input.coordinator_instance_id != service.instance_id {
                return Err(Error::WrongRig.into());
            }
            if input.allocation_id.is_nil() || input.client_id.is_nil() {
                return Err(Error::Invalid.into());
            }
            let catalogs: Vec<_> = state
                .databases
                .read()
                .map_err(|_| Error::Internal)?
                .values()
                .cloned()
                .collect();
            service
                .clone()
                .with_writer(move |store| {
                    // Lost HTTP responses retry the original grant even when its source
                    // catalog/grades/preview have since changed. No counters are rebuilt.
                    if let Some(old) = store.allocation(rig)? {
                        if old.allocation_id != input.allocation_id
                            || old.client_id != input.client_id
                            || old.catalog_id != input.catalog_id
                            || old.preview_revision != input.preview_revision
                        {
                            return Err(Error::Conflict.into());
                        }
                        return Ok(store.admit_allocation(&old)?);
                    }
                    let preview = program::assemble(
                        store,
                        &catalogs,
                        service.instance_id,
                        rig,
                        input.catalog_id,
                    )?;
                    if preview.revision != input.preview_revision {
                        return Err(Error::Conflict.into());
                    }
                    let client = store
                        .clients(rig)?
                        .into_iter()
                        .find(|c| {
                            c.client_id == input.client_id && c.catalog_id == input.catalog_id
                        })
                        .ok_or(Error::Missing)?;
                    let mut snapshot =
                        serde_json::to_value(preview).map_err(|_| Error::Internal)?;
                    // An allocation cannot accidentally reopen a ledger created while
                    // inspecting the program preview, even if all recipes match.
                    snapshot["program"]["assignment"]["id"] =
                        serde_json::json!(format!("allocation-{}", input.allocation_id));
                    snapshot["revision"] = serde_json::json!(catalog_discovery::digest(
                        &serde_json::to_vec(&snapshot).map_err(|_| Error::Internal)?
                    ));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|d| u64::try_from(d.as_millis()).ok())
                        .ok_or(Error::Internal)?;
                    let allocation = Allocation {
                        schema_version: 1,
                        allocation_id: input.allocation_id,
                        coordinator_instance_id: service.instance_id,
                        catalog_id: input.catalog_id,
                        rig_id: rig,
                        client_id: client.client_id,
                        profile_id: client.profile_id,
                        preview_revision: input.preview_revision,
                        admitted_at_ms: now,
                        snapshot,
                    };
                    Ok::<_, program::PullError>(store.admit_allocation(&allocation)?)
                })
                .await
        }
        .await,
    )
}
