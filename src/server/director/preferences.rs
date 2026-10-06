use super::*;
use crate::server::auth::RequestAccess;
use axum::Extension;
use psf_guard_director_core::priority::{Policy, Preset, Scope};
use psf_guard_director_meta::preferences::{Effective, Settings};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/preferences", get(defaults))
        .route("/preferences/{scope}/{id}", get(read).put(save))
        .route("/rigs/{rig}/preferences", get(effective))
        .layer(axum::extract::DefaultBodyLimit::max(
            psf_guard_director_meta::preferences::MAX_SETTINGS_BYTES,
        ))
}

#[derive(Serialize)]
struct Defaults {
    global_id: Uuid,
    presets: std::collections::BTreeMap<String, Policy>,
    sites: Vec<NamedIdentity>,
}
async fn defaults(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Defaults>>, Error> {
    let service = enabled(&state)?;
    let sites = service.clone().query(|s| s.sites(None, 200)).await?.items;
    Ok(Json(ApiResponse::success(Defaults {
        global_id: service.instance_id,
        presets: [
            ("balanced".into(), Policy::preset(Preset::Balanced)),
            ("finish_goals".into(), Policy::preset(Preset::FinishGoals)),
            (
                "best_conditions".into(),
                Policy::preset(Preset::BestConditions),
            ),
        ]
        .into(),
        sites,
    })))
}

async fn read(
    State(state): State<Arc<AppState>>,
    Path((scope, id)): Path<(Scope, Uuid)>,
) -> Result<Json<ApiResponse<Settings>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .query(move |s| s.observing_settings(scope, id))
            .await?,
    )))
}

async fn save(
    State(state): State<Arc<AppState>>,
    Path((scope, id)): Path<(Scope, Uuid)>,
    Extension(access): Extension<RequestAccess>,
    Json(settings): Json<Settings>,
) -> Response {
    if let Err(response) = pairing::operator(&access) {
        return *response;
    }
    let result = async {
        if scope != settings.scope || id != settings.scope_id {
            return Err(Error::Invalid);
        }
        enabled(&state)?
            .run(move |s| s.save_observing_settings(&settings))
            .await
    }
    .await;
    match result {
        Ok(saved) => Json(ApiResponse::success(saved)).into_response(),
        Err(e) => e.into_response(),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryArgs {
    project_id: Option<Uuid>,
}
async fn effective(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Query(query): Query<QueryArgs>,
) -> Result<Json<ApiResponse<Effective>>, Error> {
    Ok(Json(ApiResponse::success(
        enabled(&state)?
            .query(move |s| s.effective_observing_preferences(rig, query.project_id))
            .await?,
    )))
}
