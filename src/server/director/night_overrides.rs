//! A Director rig's night overrides over HTTP: read a night's list or a
//! range of nights, replace a night's list, and return a night to the rig's
//! own program. A change reaches a running session at its next check-in,
//! when the program it pulls carries it. A Target Scheduler rig refuses
//! them: nothing here writes its database.

use super::*;
use psf_guard_director_core::night::NightDate;
use psf_guard_director_meta::night_override::{
    check, NightOverride, NightOverrides, MAX_NIGHTS_PER_READ,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/rigs/{rig}/night-overrides", get(list))
        .route(
            "/rigs/{rig}/night-overrides/{date}",
            get(read).put(save).delete(clear),
        )
        .layer(DefaultBodyLimit::max(64 * 1024))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListQuery {
    from: NightDate,
    to: NightDate,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Save {
    expected_revision: u64,
    entries: Vec<NightOverride>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Clear {
    expected_revision: u64,
}

/// A rig's overrides over a range of nights, and whether it takes them.
#[derive(Serialize)]
struct Listed {
    runtime: night_plans::Runtime,
    nights: Vec<NightOverrides>,
}

/// One night's overrides, as an editor starts from them.
#[derive(Serialize)]
struct Night {
    date: NightDate,
    /// Send this back with a change: 0 while the night has none.
    revision: u64,
    entries: Vec<NightOverride>,
    runtime: night_plans::Runtime,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn runtime(store: &MetaStore, rig: Uuid) -> Result<night_plans::Runtime, Error> {
    store.rig(rig)?.ok_or(Error::Missing)?;
    let profile = store.rig_profile(rig)?;
    Ok(night_plans::runtime_of(store, rig, profile.as_ref())?)
}

async fn list(
    State(state): State<Arc<AppState>>,
    Path(rig): Path<Uuid>,
    Query(query): Query<ListQuery>,
) -> Result<Json<ApiResponse<Listed>>, Error> {
    if query.from > query.to {
        return Err(Error::Invalid);
    }
    if query.from.days_until(query.to) >= MAX_NIGHTS_PER_READ as i64 {
        return Err(Error::Refused(format!(
            "Ask for fewer nights: one read covers at most {MAX_NIGHTS_PER_READ}."
        )));
    }
    let listed = enabled(&state)?
        .with_reader(move |store| {
            Ok::<_, Error>(Listed {
                runtime: runtime(store, rig)?,
                nights: store.night_overrides(rig, query.from, query.to)?,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(listed)))
}

async fn read(
    State(state): State<Arc<AppState>>,
    Path((rig, date)): Path<(Uuid, NightDate)>,
) -> Result<Json<ApiResponse<Night>>, Error> {
    let night = enabled(&state)?
        .with_reader(move |store| {
            let runtime = runtime(store, rig)?;
            let stored = store.night_override(rig, date)?;
            Ok::<_, Error>(Night {
                date,
                revision: stored.as_ref().map_or(0, |night| night.revision),
                entries: stored.map(|night| night.entries).unwrap_or_default(),
                runtime,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(night)))
}

async fn save(
    State(state): State<Arc<AppState>>,
    Path((rig, date)): Path<(Uuid, NightDate)>,
    Json(body): Json<Save>,
) -> Result<Json<ApiResponse<Night>>, Error> {
    write(&state, rig, date, body.entries, body.expected_revision).await
}

async fn clear(
    State(state): State<Arc<AppState>>,
    Path((rig, date)): Path<(Uuid, NightDate)>,
    Json(body): Json<Clear>,
) -> Result<Json<ApiResponse<Night>>, Error> {
    write(&state, rig, date, Vec::new(), body.expected_revision).await
}

async fn write(
    state: &Arc<AppState>,
    rig: Uuid,
    date: NightDate,
    entries: Vec<NightOverride>,
    expected_revision: u64,
) -> Result<Json<ApiResponse<Night>>, Error> {
    let night = enabled(state)?
        .with_writer(move |store| {
            let runtime = runtime(store, rig)?;
            if runtime != night_plans::Runtime::Director {
                return Err(Error::Mode(NOT_DIRECTOR.into()));
            }
            let profile = store.rig_profile(rig)?;
            let location = store.rig_site(rig, profile.as_ref())?.location.ok_or_else(|| {
                Error::Refused(
                    "Give the rig a location, or a planning site with one, before overriding its nights.".into(),
                )
            })?;
            let span = date.span(location.longitude_degrees);
            if !entries.is_empty() && span.end_ms <= now_ms() {
                return Err(Error::Refused(format!(
                    "The night of {date} has ended, so overrides no longer apply to it."
                )));
            }
            check(&entries, span).map_err(|refusal| Error::Refused(refusal.to_string()))?;
            let saved =
                store.save_night_overrides(rig, date, &entries, expected_revision, now_ms())?;
            Ok(Night {
                date,
                revision: saved.as_ref().map_or(0, |night| night.revision),
                entries: saved.map(|night| night.entries).unwrap_or_default(),
                runtime,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(night)))
}
