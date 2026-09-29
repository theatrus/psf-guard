//! The exposure template library over HTTP: Director's own templates,
//! shared by every rig. A plan may bind a rig to one of these instead of
//! a template in the rig's database; activation then writes it into that
//! database. Reads and drafts are open to every server, like plans.

use super::*;
use psf_guard_director_core::bandpass::{bandpass_for_filter, Bandpass};
use psf_guard_director_meta::templates::ExposureTemplate;
use std::time::{SystemTime, UNIX_EPOCH};

/// A library template with the bandpass its filter name resolves to.
#[derive(Serialize)]
pub(super) struct LibraryTemplate {
    #[serde(flatten)]
    template: ExposureTemplate,
    bandpass: Bandpass,
}

fn view(template: ExposureTemplate) -> LibraryTemplate {
    LibraryTemplate {
        bandpass: bandpass_for_filter(&template.filter_name),
        template,
    }
}

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/templates", get(list))
        .route("/templates/{id}", axum::routing::put(save).delete(remove))
}

async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<LibraryTemplate>>>, Error> {
    let templates = enabled(&state)?.query(|store| store.templates()).await?;
    Ok(Json(ApiResponse::success(
        templates.into_iter().map(view).collect(),
    )))
}

/// The body is the whole template; its `revision` is the one the caller
/// read, 0 for a new one.
async fn save(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(mut template): Json<ExposureTemplate>,
) -> Result<Json<ApiResponse<LibraryTemplate>>, Error> {
    if template.id != id {
        return Err(Error::Invalid);
    }
    template.updated_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let saved = enabled(&state)?
        .run(move |store| {
            let expected = template.revision;
            store.save_template(&template, expected)
        })
        .await?;
    Ok(Json(ApiResponse::success(view(saved))))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RemoveQuery {
    /// The revision the caller read, so nobody deletes what they have not seen.
    revision: u64,
}

async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Query(query): Query<RemoveQuery>,
) -> Result<Json<ApiResponse<serde_json::Value>>, Error> {
    enabled(&state)?
        .run(move |store| store.delete_template(id, query.revision))
        .await?;
    Ok(Json(ApiResponse::success(
        serde_json::json!({ "deleted": true }),
    )))
}
