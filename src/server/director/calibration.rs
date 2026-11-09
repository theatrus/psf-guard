//! Calibration recipes are settings, never an allocation or permission to move.
//! Flat coverage shares the operator invalidation UI and replay rules with Sync.
use super::*;
use crate::server::flat_history::{self, AcknowledgeResult, CaptureRecord, Scope, SnapshotRequest};
use axum::http::{header::AUTHORIZATION, HeaderMap};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/calibration", axum::routing::post(exchange))
        .layer(DefaultBodyLimit::max(flat_history::MAX_BODY_BYTES))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Exchange {
    protocol_version: u32,
    coordinator_instance_id: Uuid,
    catalog_id: Uuid,
    records: Vec<CaptureRecord>,
    acknowledgements: Vec<AcknowledgeResult>,
}

async fn exchange(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    headers: HeaderMap,
    Json(input): Json<Exchange>,
) -> Result<Response, program::PullError> {
    let service = enabled(&state)?;
    if input.protocol_version != 1 || input.coordinator_instance_id != service.instance_id {
        return Err(Error::WrongRig.into());
    }
    // Require the paired native client even if a browser cookie is present.
    let secret = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split_once(' '))
        .filter(|(scheme, token)| {
            scheme.eq_ignore_ascii_case("bearer") && token.starts_with("psfdrc_")
        })
        .map(|(_, token)| token)
        .ok_or(Error::Invalid)?;
    let hash = crate::auth_registry::hash_token(secret);
    let profile = headers
        .get("x-psf-director-profile")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| Uuid::parse_str(v).ok())
        .ok_or(Error::Invalid)?;
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let instance = service.instance_id;
    let catalog = input.catalog_id;
    let (ctx, configuration, recipes) = service
        .with_reader(move |store| {
            let client = store.client_for_token(&hash)?.ok_or(Error::Invalid)?;
            if client.rig_id != rig || client.catalog_id != catalog || client.profile_id != profile
            {
                return Err(Error::WrongRig.into());
            }
            let gathered = program::gather(store, &catalogs, instance, rig, catalog, false)?;
            let ctx = identified_catalogs(&catalogs, instance)
                .get(catalog)
                .cloned()
                .ok_or(Error::Missing)?;
            let recipes = gathered.calibration_recipes().to_vec();
            Ok::<_, program::PullError>((ctx, gathered.configuration, recipes))
        })
        .await?;
    // The origin is server-selected and survives a replacement pairing token.
    // Profile coverage is deliberately not attributed to a guessed target.
    if input.records.iter().any(|r| {
        r.profile_id != profile.to_string()
            || r.target_guid.is_some()
            || r.flats_type.as_deref() != Some("Director panel")
    }) {
        return Err(Error::WrongRig.into());
    }
    let scope = Scope {
        protocol_version: 1,
        // The paired catalog UUID selected this exact context above. The shared
        // flat-history store uses its registry slug, not the Director UUID.
        catalog_id: ctx.id.clone(),
        origin_id: profile.to_string(),
    };
    let result = flat_history::director_exchange(
        ctx,
        SnapshotRequest {
            scope,
            source_name: "Director".into(),
            records: input.records,
        },
        input.acknowledgements,
    )
    .await;
    let (received, acknowledged, decisions, retired) = match result {
        Ok(result) => result,
        Err(error) => return Ok(pairing::no_store(error.into_response())),
    };
    Ok(pairing::no_store(Json(ApiResponse::success(
        serde_json::json!({
            "protocol_version": 1, "coordinator_instance_id": instance, "catalog_id": catalog,
            "rig_id": rig, "profile_id": profile, "origin_id": profile,
            "configuration": configuration, "recipes": recipes,
            "received": received, "acknowledged": acknowledged, "decisions": decisions, "retired": retired,
        }),
    ))))
}
