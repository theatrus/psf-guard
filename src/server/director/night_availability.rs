//! Night availability marks over HTTP: read a site's or a rig's marks, mark
//! one night or a range, and return nights to the default. Each scope's
//! marks change together under one compare-and-set revision. Planning input
//! only: marks never reach a program, the plugin or Target Scheduler.

use super::*;
use psf_guard_director_core::night::NightDate;
use psf_guard_director_meta::night_availability::{
    MarkEdit, MarkScope, NightMarks, MAX_NIGHTS_PER_READ,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/night-availability", get(list))
        .route(
            "/night-availability/{scope}/{id}",
            axum::routing::put(save).delete(clear),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    scope: MarkScope,
    id: Uuid,
    from: NightDate,
    to: NightDate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    expected_revision: u64,
    marks: Vec<MarkEdit>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Clear {
    expected_revision: u64,
    dates: Vec<NightDate>,
}

#[derive(Serialize)]
struct Cleared {
    revision: u64,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn scope_of(text: &str) -> Result<MarkScope, Error> {
    text.parse().map_err(|_| Error::Invalid)
}

async fn list(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<NightMarks>>, Error> {
    if query.from > query.to {
        return Err(Error::Invalid);
    }
    if query.from.days_until(query.to) >= MAX_NIGHTS_PER_READ as i64 {
        return Err(Error::Refused(format!(
            "Ask for fewer nights: one read covers at most {MAX_NIGHTS_PER_READ}."
        )));
    }
    let marks = enabled(&state)?
        .query(move |store| store.night_marks(query.scope, query.id, query.from, query.to))
        .await?;
    Ok(Json(ApiResponse::success(marks)))
}

async fn save(
    State(state): State<Arc<AppState>>,
    Path((scope, id)): Path<(String, Uuid)>,
    Json(body): Json<Save>,
) -> Result<Json<ApiResponse<NightMarks>>, Error> {
    let scope = scope_of(&scope)?;
    let saved = enabled(&state)?
        .with_writer(move |store| {
            // A limited night's windows must fall inside that night, which
            // needs the scope's longitude.
            if body.marks.iter().any(|mark| !mark.windows.is_empty()) {
                let longitude = longitude(store, scope, id)?.ok_or_else(|| {
                    Error::Refused(match scope {
                        MarkScope::Site => "Give the site a location before marking hours.".into(),
                        MarkScope::Rig => "Give the rig a location before marking hours.".into(),
                    })
                })?;
                for mark in &body.marks {
                    let span = mark.date.span(longitude);
                    if mark
                        .windows
                        .iter()
                        .any(|w| w.start_ms < span.start_ms || w.end_ms > span.end_ms)
                    {
                        return Err(Error::Refused(format!(
                            "The hours marked for {} fall outside that night, which runs from noon to noon.",
                            mark.date
                        )));
                    }
                }
            }
            Ok::<_, Error>(store.save_night_marks(scope, id, body.expected_revision, &body.marks, now_ms())?)
        })
        .await?;
    Ok(Json(ApiResponse::success(saved)))
}

async fn clear(
    State(state): State<Arc<AppState>>,
    Path((scope, id)): Path<(String, Uuid)>,
    Json(body): Json<Clear>,
) -> Result<Json<ApiResponse<Cleared>>, Error> {
    let scope = scope_of(&scope)?;
    let revision = enabled(&state)?
        .run(move |store| store.clear_night_marks(scope, id, body.expected_revision, &body.dates))
        .await?;
    Ok(Json(ApiResponse::success(Cleared { revision })))
}

/// The longitude a scope's nights are keyed by: the site's location, or the
/// rig's planning location. `None` when it has none yet.
fn longitude(store: &MetaStore, scope: MarkScope, id: Uuid) -> Result<Option<f64>, StoreError> {
    Ok(match scope {
        MarkScope::Site => store
            .site_profile(id)?
            .and_then(|profile| profile.location)
            .map(|location| location.value.longitude_degrees),
        MarkScope::Rig => {
            let profile = store.rig_profile(id)?;
            store
                .rig_site(id, profile.as_ref())?
                .location
                .map(|location| location.longitude_degrees)
        }
    })
}
