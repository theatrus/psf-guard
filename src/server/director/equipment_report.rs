//! Native evidence may be submitted by a paired client; only an interactive
//! operator can adopt it into the existing rig setup after explicit review.
use super::*;
use axum::{
    http::{
        header::{AUTHORIZATION, CACHE_CONTROL},
        HeaderMap, HeaderValue,
    },
    Extension,
};
use psf_guard_director_core::program::Configuration;
use psf_guard_director_meta::{equipment_report::EquipmentReport, profile::RigProfile};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Submit {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    report_id: Uuid,
    observed_at_ms: u64,
    configuration: Configuration,
    filter_names: BTreeMap<String, String>,
}

#[derive(Serialize)]
pub(super) struct Acknowledgement {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    rig_id: Uuid,
    profile_id: Uuid,
    client_id: Uuid,
    report_id: Uuid,
    received_at_ms: u64,
    accepted_revision: Option<u64>,
}

fn now_ms() -> Result<u64, Error> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_millis()).ok())
        .ok_or(Error::Internal)
}

pub(super) async fn submit(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<Submit>,
) -> Result<Response, Error> {
    let service = enabled(&state)?;
    if input.coordinator_instance_id != service.instance_id
        || input.configuration.rig_id != rig.to_string()
    {
        return Err(Error::WrongRig);
    }
    // Do not let an operator cookie stand in for a native executor credential.
    let secret = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(s, _)| s.eq_ignore_ascii_case("bearer"))
        .map(|(_, s)| s.trim())
        .filter(|s| s.starts_with("psfdrc_"))
        .ok_or(Error::Invalid)?;
    let hash = crate::auth_registry::hash_token(secret);
    let profile = headers
        .get("x-psf-director-profile")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(Error::Invalid)?;
    let instance = service.instance_id;
    let answer = service
        .with_writer(move |store| {
            let client = store.client_for_token(&hash)?.ok_or(Error::Invalid)?;
            if client.rig_id != rig
                || client.catalog_id != input.catalog_id
                || client.profile_id != profile
            {
                return Err(Error::WrongRig);
            }
            let report = store.report_equipment(&EquipmentReport {
                report_id: input.report_id,
                client_id: client.client_id,
                catalog_id: input.catalog_id,
                rig_id: rig,
                profile_id: profile,
                observed_at_ms: input.observed_at_ms,
                received_at_ms: now_ms()?,
                configuration: input.configuration,
                filter_names: input.filter_names,
                accepted_revision: None,
                accepted_from_revision: None,
            })?;
            Ok::<_, Error>(Acknowledgement {
                coordinator_instance_id: instance,
                catalog_id: report.catalog_id,
                rig_id: rig,
                profile_id: profile,
                client_id: client.client_id,
                report_id: report.report_id,
                received_at_ms: report.received_at_ms,
                accepted_revision: report.accepted_revision,
            })
        })
        .await?;
    Ok(no_store(Json(ApiResponse::success(answer)).into_response()))
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
) -> Result<Response, Error> {
    let value = enabled(&state)?
        .query(move |store| store.equipment_reports(rig))
        .await?;
    Ok(no_store(Json(ApiResponse::success(value)).into_response()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Accept {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    report_id: Uuid,
    expected_revision: u64,
}

pub(super) async fn accept(
    State(state): State<Arc<AppState>>,
    Path((rig, client)): Path<(Uuid, Uuid)>,
    Extension(access): Extension<crate::server::auth::RequestAccess>,
    Json(input): Json<Accept>,
) -> Response {
    let result = async {
        pairing::operator(&access).map_err(|r| *r)?;
        let service = enabled(&state).map_err(IntoResponse::into_response)?;
        if input.coordinator_instance_id != service.instance_id {
            return Err(Error::WrongRig.into_response());
        }
        let value = service
            .with_writer(move |store| {
                if !store
                    .catalog_rig(input.catalog_id)?
                    .is_some_and(|b| b.rig.id == rig)
                {
                    return Err(Error::WrongRig);
                }
                let report = store
                    .equipment_reports(rig)?
                    .into_iter()
                    .find(|r| r.client_id == client)
                    .ok_or(Error::Missing)?;
                if report.catalog_id != input.catalog_id {
                    return Err(Error::WrongRig);
                }
                store
                    .accept_equipment_report(
                        rig,
                        client,
                        input.report_id,
                        input.expected_revision,
                        now_ms()?,
                    )
                    .map_err(Error::from)
            })
            .await
            .map_err(IntoResponse::into_response)?;
        Ok::<Json<ApiResponse<RigProfile>>, Response>(Json(ApiResponse::success(value)))
    }
    .await;
    match result {
        Ok(value) => no_store(value.into_response()),
        Err(error) => no_store(error),
    }
}

fn no_store(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
