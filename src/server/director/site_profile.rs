//! A site's location, horizon and forecast choice, edited in Setup and
//! inherited by every rig that names the site as its planning site. Horizons
//! arrive as N.I.N.A. `.hrz` text and are checked by the shared core before
//! they are stored.

use super::*;
use psf_guard_director_core::visibility::{Horizon, Site};
use psf_guard_director_meta::{
    profile::{Reported, Source},
    site_forecast::ForecastSettings,
    site_profile::SiteProfile,
};
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sites/{site}/profile", get(get_profile).put(put_profile))
        .route("/horizons/parse", axum::routing::post(parse))
        .layer(DefaultBodyLimit::max(
            psf_guard_director_core::MAX_REQUEST_BYTES,
        ))
}

#[derive(Serialize)]
struct ProfileView {
    site: NamedIdentity,
    profile: SiteProfile,
}

/// An operator's edit. Timestamps are stamped by the server.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edit {
    expected_revision: u64,
    location: Option<Edited<Site>>,
    horizon: Option<Edited<Horizon>>,
    /// The site's cloud forecast provider and threshold. Left out, forecasts
    /// are off for the site, and PSF Guard sends nothing about it.
    #[serde(default)]
    forecast: Option<ForecastSettings>,
    /// The site's IANA time zone, for entering and showing marks and
    /// overrides. Left out, the browser's zone is used.
    #[serde(default)]
    time_zone: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Edited<T> {
    value: T,
    source: Source,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn get_profile(
    State(state): State<Arc<AppState>>,
    Path(site): Path<Uuid>,
) -> Result<Json<ApiResponse<ProfileView>>, Error> {
    let view = enabled(&state)?
        .query(move |store| {
            let identity = store.site(site)?.ok_or(StoreError::NotFound)?;
            let profile = store
                .site_profile(site)?
                .unwrap_or_else(|| SiteProfile::empty(site, now_ms()));
            Ok::<_, StoreError>(ProfileView {
                site: identity,
                profile,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(view)))
}

async fn put_profile(
    State(state): State<Arc<AppState>>,
    Path(site): Path<Uuid>,
    Json(edit): Json<Edit>,
) -> Result<Json<ApiResponse<ProfileView>>, Error> {
    // Only the plugin reports, and it reports rigs, not sites.
    if [
        edit.location.as_ref().map(|part| &part.source),
        edit.horizon.as_ref().map(|part| &part.source),
    ]
    .into_iter()
    .flatten()
    .any(|source| matches!(source, Source::Plugin {}))
    {
        return Err(Error::Invalid);
    }
    let time_zone = edit
        .time_zone
        .map(|zone| zone.trim().to_owned())
        .filter(|zone| !zone.is_empty());
    if time_zone
        .as_deref()
        .is_some_and(|zone| !psf_guard_director_meta::site_profile::time_zone_name(zone))
    {
        return Err(Error::Refused(
            "Choose the site's time zone from the list, such as America/Los_Angeles.".into(),
        ));
    }
    if edit.forecast.is_some() && edit.location.is_none() {
        return Err(Error::Refused(
            "A forecast needs the site's location. Enter its latitude and longitude.".into(),
        ));
    }
    let service = enabled(&state)?;
    let (view, refresh) = service
        .clone()
        .run(move |store| {
            let identity = store.site(site)?.ok_or(StoreError::NotFound)?;
            let now = now_ms();
            let stored = store.site_profile(site)?;
            let previous = stored.as_ref();
            let mut next = stored
                .clone()
                .unwrap_or_else(|| SiteProfile::empty(site, now));
            next.location = stamp(
                edit.location,
                previous.and_then(|p| p.location.as_ref()),
                now,
            );
            next.horizon = stamp(edit.horizon, previous.and_then(|p| p.horizon.as_ref()), now);
            next.forecast = edit.forecast;
            next.time_zone = time_zone;
            next.updated_at_ms = now;
            let profile = store.save_site_profile(&next, edit.expected_revision)?;
            // A forecast is for one provider and one place, to two
            // decimals: a change to either, or no forecast at all, discards
            // the stored one. A new elevation or threshold keeps it.
            let before = previous.and_then(super::site_forecast::request_key);
            let after = super::site_forecast::request_key(&profile);
            if before != after {
                store.clear_site_forecast(site)?;
            }
            Ok::<_, StoreError>((
                ProfileView {
                    site: identity,
                    profile,
                },
                after.is_some() && before != after,
            ))
        })
        .await?;
    if refresh {
        super::site_forecast::settings_changed(&state, service, site);
    }
    Ok(Json(ApiResponse::success(view)))
}

/// Keep the old timestamp when the part did not change.
fn stamp<T: PartialEq>(
    edited: Option<Edited<T>>,
    previous: Option<&Reported<T>>,
    now: u64,
) -> Option<Reported<T>> {
    let edited = edited?;
    let reported_at_ms = match previous {
        Some(previous) if previous.value == edited.value && previous.source == edited.source => {
            previous.reported_at_ms
        }
        _ => now,
    };
    Some(Reported {
        value: edited.value,
        source: edited.source,
        reported_at_ms,
    })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct HorizonText {
    text: String,
}

/// Read a pasted or uploaded `.hrz` file into the canonical curve, or say
/// which line is wrong. Nothing is stored.
async fn parse(Json(body): Json<HorizonText>) -> Response {
    match Horizon::from_hrz(&body.text) {
        Ok(horizon) => Json(ApiResponse::success(horizon)).into_response(),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(ApiResponse::<()>::error(format!(
                "Horizon file not read: {error}."
            ))),
        )
            .into_response(),
    }
}
