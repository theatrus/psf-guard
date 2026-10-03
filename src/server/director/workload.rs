//! Automatic grant intake under an interactive, reviewed commissioning policy.
use super::*;
use crate::server::auth::RequestAccess;
use axum::{
    http::{header::CACHE_CONTROL, HeaderValue},
    Extension,
};
use psf_guard_director_meta::{
    allocation::Allocation,
    workload::{Policy, Workload},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Commission {
    coordinator_instance_id: Uuid,
    expected_revision: u64,
    policy: Policy,
}

fn response<T: serde::Serialize>(value: Result<T, program::PullError>) -> Response {
    let mut r = match value {
        Ok(v) => Json(ApiResponse::success(v)).into_response(),
        Err(e) => e.into_response(),
    };
    r.headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    r
}

pub(super) async fn commission(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    Json(input): Json<Commission>,
) -> Response {
    if let Err(r) = pairing::operator(&access) {
        return *r;
    }
    response(
        async {
            let service = enabled(&state)?;
            if input.coordinator_instance_id != service.instance_id || input.policy.rig_id != rig {
                return Err(Error::WrongRig.into());
            }
            service
                .with_writer(move |s| {
                    Ok::<_, program::PullError>(
                        s.save_workload_policy(&input.policy, input.expected_revision)?,
                    )
                })
                .await
        }
        .await,
    )
}

pub(super) async fn get_policy(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
) -> Response {
    if let Err(r) = pairing::operator(&access) {
        return *r;
    }
    response(
        async {
            Ok::<_, program::PullError>(
                enabled(&state)?
                    .query(move |s| s.workload_policy(rig))
                    .await?,
            )
        }
        .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Request {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    request_id: Uuid,
    configuration_id: String,
    execution_mode: ExecutionMode,
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum ExecutionMode {
    PreparedTargetV1,
    LocalSequenceV1,
    PreparedTargetV2,
    LocalSequenceV2,
    PreparedTargetV3,
    LocalSequenceV3,
}

pub(super) fn supports_prepared_target(p: &psf_guard_director_core::program::Program) -> bool {
    p.targets.len() == 1 && supports_local_sequence(p)
}

pub(super) fn supports_local_sequence(p: &psf_guard_director_core::program::Program) -> bool {
    !p.targets.is_empty()
        && p.targets.iter().all(|t| t.position_angle_mas.is_none())
        && !p.configuration.enable_slew_center
        && p.configuration.dither_every == 0
        && p.recipes
            .iter()
            .all(|r| r.dither_override.is_none_or(|n| n == 0))
}

impl ExecutionMode {
    pub(super) fn validate(
        &self,
        p: &psf_guard_director_core::program::Program,
    ) -> Result<(), program::PullError> {
        let moon_required = p
            .recipes
            .iter()
            .any(|r| r.moon.as_ref().is_some_and(|m| m.enabled));
        let supported = match self {
            Self::PreparedTargetV1 => {
                p.observing_preferences.is_none() && !moon_required && supports_prepared_target(p)
            }
            Self::LocalSequenceV1 => {
                p.observing_preferences.is_none() && !moon_required && supports_local_sequence(p)
            }
            Self::PreparedTargetV2 => {
                p.observing_preferences.is_none() && supports_prepared_target(p)
            }
            Self::LocalSequenceV2 => {
                p.observing_preferences.is_none() && supports_local_sequence(p)
            }
            Self::PreparedTargetV3 => supports_prepared_target(p),
            Self::LocalSequenceV3 => supports_local_sequence(p),
        };
        if supported {
            Ok(())
        } else {
            Err(program::PullError::NotReady(
                "The workload exceeds the executor mode's target/rotation, Moon avoidance, observing preferences or sequence-owned preparation capabilities. No new workload was issued.".into(),
            ))
        }
    }
}

#[derive(Serialize)]
struct Reply {
    request_id: Uuid,
    state: &'static str,
    workload: Option<Workload>,
    retry_after_seconds: u32,
}

fn credential(headers: &axum::http::HeaderMap) -> Result<(String, Uuid), Error> {
    let secret = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(s, _)| s.eq_ignore_ascii_case("bearer"))
        .map(|(_, s)| s.trim())
        .filter(|s| s.starts_with("psfdrc_"))
        .ok_or(Error::Invalid)?;
    let profile = headers
        .get("x-psf-director-profile")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(Error::Invalid)?;
    Ok((crate::auth_registry::hash_token(secret), profile))
}

pub(super) async fn request(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<Request>,
) -> Response {
    response(
        async {
            let service = enabled(&state)?;
            if input.coordinator_instance_id != service.instance_id || input.request_id.is_nil() {
                return Err(Error::WrongRig.into());
            }
            let (hash, profile) = credential(&headers)?;
            let catalogs = state
                .databases
                .read()
                .map_err(|_| Error::Internal)?
                .values()
                .cloned()
                .collect::<Vec<_>>();
            service
                .clone()
                .with_writer(move |store| {
                    let c = store.client_for_token(&hash)?.ok_or(Error::Invalid)?;
                    if c.rig_id != rig
                        || c.catalog_id != input.catalog_id
                        || c.profile_id != profile
                    {
                        return Err(Error::WrongRig.into());
                    }
                    if let Some(w) = store.workload(rig, c.client_id, input.request_id)? {
                        if w.allocation.snapshot["program"]["configuration"]["id"]
                            != input.configuration_id
                        {
                            return Err(Error::Conflict.into());
                        }
                        let program =
                            serde_json::from_value(w.allocation.snapshot["program"].clone())
                                .map_err(|_| Error::Internal)?;
                        input.execution_mode.validate(&program)?;
                        return Ok(Reply {
                            request_id: input.request_id,
                            state: if w.released { "released" } else { "issued" },
                            workload: Some(w),
                            retry_after_seconds: 0,
                        });
                    }
                    let p = store.workload_policy(rig)?.ok_or(Error::Conflict)?;
                    if !p.enabled
                        || p.client_id != c.client_id
                        || p.catalog_id != c.catalog_id
                        || p.profile_id != profile
                        || p.configuration_id != input.configuration_id
                    {
                        return Err(Error::Conflict.into());
                    }
                    let active = store.rig_profile(rig)?.ok_or(Error::Conflict)?;
                    if active.revision != p.profile_revision
                        || !active
                            .configuration
                            .is_some_and(|v| v.value.id == p.configuration_id)
                    {
                        return Err(Error::Conflict.into());
                    }
                    if let Some(old) = store.allocation(rig)?
                        && !store
                            .workload(rig, old.client_id, old.allocation_id)?
                            .is_some_and(|w| w.released)
                    {
                        return Err(Error::Conflict.into());
                    }
                    let mut preview = program::assemble(
                        store,
                        &catalogs,
                        service.instance_id,
                        rig,
                        input.catalog_id,
                    )?;
                    store.carry_workload_budget(rig, &mut preview.program)?;
                    if preview.program.assignment.goals.iter().all(|g| {
                        g.accepted.saturating_add(g.pending) >= g.requested
                            || g.attempts_remaining == 0
                    }) {
                        return Ok(Reply {
                            request_id: input.request_id,
                            state: "waiting",
                            workload: None,
                            retry_after_seconds: 30,
                        });
                    }
                    let revision = preview.revision.clone();
                    input.execution_mode.validate(&preview.program)?;
                    preview.program.assignment.id = format!("allocation-{}", input.request_id);
                    let mut snapshot =
                        serde_json::to_value(preview).map_err(|_| Error::Internal)?;
                    snapshot["revision"] = serde_json::json!(catalog_discovery::digest(
                        &serde_json::to_vec(&snapshot).map_err(|_| Error::Internal)?
                    ));
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()
                        .and_then(|d| u64::try_from(d.as_millis()).ok())
                        .ok_or(Error::Internal)?;
                    let grant = Allocation {
                        schema_version: 1,
                        allocation_id: input.request_id,
                        coordinator_instance_id: service.instance_id,
                        catalog_id: c.catalog_id,
                        rig_id: rig,
                        client_id: c.client_id,
                        profile_id: profile,
                        preview_revision: revision,
                        admitted_at_ms: now,
                        snapshot,
                    };
                    Ok::<_, program::PullError>(Reply {
                        request_id: input.request_id,
                        state: "issued",
                        workload: Some(store.admit_workload(&grant, p.revision)?),
                        retry_after_seconds: 0,
                    })
                })
                .await
        }
        .await,
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Release {
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    allocation_id: Uuid,
    ledger_id: Uuid,
    terminal_sequence: u64,
    operations_quiescent: bool,
    parked: bool,
}

pub(super) async fn release(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: axum::http::HeaderMap,
    Json(input): Json<Release>,
) -> Response {
    response(
        async {
            let service = enabled(&state)?;
            if input.coordinator_instance_id != service.instance_id
                || !input.operations_quiescent
                || !input.parked
            {
                return Err(Error::Conflict.into());
            }
            let (hash, profile) = credential(&headers)?;
            service
                .with_writer(move |s| {
                    let c = s.client_for_token(&hash)?.ok_or(Error::Invalid)?;
                    if c.rig_id != rig
                        || c.catalog_id != input.catalog_id
                        || c.profile_id != profile
                    {
                        return Err(Error::WrongRig.into());
                    }
                    Ok::<_, program::PullError>(s.release_workload(
                        rig,
                        c.client_id,
                        input.allocation_id,
                        input.ledger_id,
                        input.terminal_sequence,
                    )?)
                })
                .await
        }
        .await,
    )
}
