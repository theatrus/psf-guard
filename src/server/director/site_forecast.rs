//! A site's cloud forecast (design §4.3.1 of `docs/design/night-plans.md`).
//! `GET /sites/{site}/forecast` returns the stored forecast and the result
//! of its last refresh. `POST /sites/{site}/forecast/refresh` requests a new
//! forecast at once. A timer refreshes each site with forecasts on every
//! three hours. Forecasts change night plans only: they never allow or block
//! acquisition.

use super::*;
use crate::server::forecast::{FetchError, Fetched};
use psf_guard_director_core::visibility::Site;
use psf_guard_director_meta::{
    site_forecast::{
        ForecastFailure, ForecastProvider, ForecastSettings, ForecastStatus, SiteForecast,
        FORECAST_VALID_MS,
    },
    site_profile::SiteProfile,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const MINUTE_MS: u64 = 60_000;
const HOUR_MS: u64 = 60 * MINUTE_MS;
/// How long a stored forecast is used before the timer refreshes it.
const REFRESH_MS: u64 = 3 * HOUR_MS;
/// The wait after a first failure, doubling up to `MAX_BACKOFF_MS`.
const FIRST_BACKOFF_MS: u64 = 15 * MINUTE_MS;
const MAX_BACKOFF_MS: u64 = 6 * HOUR_MS;
/// How soon after a forecast arrives Fetch now may request another.
const MIN_GAP_MS: u64 = 2 * MINUTE_MS;
/// How soon after a failed request Fetch now may try again.
const RETRY_GAP_MS: u64 = MINUTE_MS;
/// How soon a forecast stored without the air is asked for again while
/// the site has a dew limit: a forecast from before PSF Guard asked for
/// the air.
const AIR_REFRESH_MS: u64 = 30 * MINUTE_MS;
/// How often the timer looks for sites that are due.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

pub(super) fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sites/{site}/forecast", get(show))
        .route(
            "/sites/{site}/forecast/refresh",
            axum::routing::post(refresh_now),
        )
}

/// Refresh each site's forecast when it is due, while the server runs.
pub(crate) fn spawn(state: &Arc<AppState>) {
    crate::server::schedule::every(
        state,
        "site forecast",
        crate::server::schedule::Every::after(Duration::from_secs(60), CHECK_EVERY),
        |state| async move { refresh_due(&state).await },
    );
}

/// A site's forecast as its setup shows it.
#[derive(Serialize)]
struct ForecastView {
    site: NamedIdentity,
    settings: Option<ForecastSettings>,
    /// The credit the provider's licence requires.
    attribution: Option<&'static str>,
    /// The stored forecast, fresh or not.
    forecast: Option<SiteForecast>,
    /// Whether night plans use the stored forecast now.
    fresh: bool,
    status: Option<ForecastStatus>,
    /// When the timer next refreshes the forecast.
    next_refresh_ms: Option<u64>,
}

/// What the meta store records about a site's forecast.
struct Kept {
    site: NamedIdentity,
    settings: Option<ForecastSettings>,
    location: Option<Site>,
    forecast: Option<SiteForecast>,
    status: Option<ForecastStatus>,
}

impl Kept {
    fn read(store: &MetaStore, site: Uuid) -> Result<Self, StoreError> {
        let identity = store.site(site)?.ok_or(StoreError::NotFound)?;
        let profile = store.site_profile(site)?;
        Ok(Self {
            site: identity,
            settings: profile.as_ref().and_then(|p| p.forecast),
            location: profile.and_then(|p| p.location.map(|l| l.value)),
            forecast: store.site_forecast(site)?,
            status: store.forecast_status(site)?,
        })
    }

    /// The stored forecast, when it came from the site's current provider.
    fn current(&self) -> Option<&SiteForecast> {
        let provider = self.settings?.provider;
        self.forecast.as_ref().filter(|f| f.provider == provider)
    }

    /// When Fetch now may next send a request, and why not sooner.
    fn manual_after(&self, now: u64) -> Option<(u64, String)> {
        let provider = self.settings.map_or("The provider", |s| s.provider.label());
        let mut waits = Vec::new();
        if let Some(forecast) = self.current() {
            // A time from a clock that ran ahead is no reason to wait.
            let confirmed = forecast.checked_at_ms.unwrap_or(forecast.fetched_at_ms);
            if confirmed <= now {
                waits.push((
                    confirmed + MIN_GAP_MS,
                    "The forecast was refreshed less than 2 minutes ago.".to_owned(),
                ));
            }
            if let Some(after) = forecast.refresh_after_ms {
                waits.push((after, format!("{provider}'s terms allow no new request until the current forecast expires.")));
            }
        }
        if let Some(status) = &self.status
            && status.last_error.is_some()
        {
            if status.last_attempt_ms <= now {
                waits.push((
                    status.last_attempt_ms + RETRY_GAP_MS,
                    "The last request failed less than a minute ago.".to_owned(),
                ));
            }
            if let Some(after) = status.retry_after_ms {
                waits.push((
                    after,
                    format!("{provider} asked PSF Guard to wait before its next request."),
                ));
            }
        }
        waits
            .into_iter()
            .filter(|(at, _)| *at > now)
            .max_by_key(|(at, _)| *at)
    }

    fn view(self, now: u64) -> ForecastView {
        let next_refresh_ms = self.settings.map(|s| {
            next_due(
                s.provider,
                s.dew_limit_celsius.is_some(),
                self.forecast.as_ref(),
                self.status.as_ref(),
                now,
            )
            .max(now)
        });
        let provider = self
            .settings
            .map(|s| s.provider)
            .or(self.forecast.as_ref().map(|f| f.provider));
        ForecastView {
            fresh: self.current().is_some_and(|f| f.fresh(now)),
            site: self.site,
            settings: self.settings,
            attribution: provider.map(ForecastProvider::attribution),
            forecast: self.forecast,
            status: self.status,
            next_refresh_ms,
        }
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// What a forecast is requested for: the provider and the coordinates as
/// sent, rounded to two decimals. A change to anything else, such as the
/// elevation, keeps the stored forecast.
pub(super) fn request_key(profile: &SiteProfile) -> Option<(ForecastProvider, String, String)> {
    let location = profile.location.as_ref()?.value;
    let (latitude, longitude) = psf_guard_director_meta::site_forecast::forecast_coordinates(
        location.latitude_degrees,
        location.longitude_degrees,
    );
    Some((profile.forecast?.provider, latitude, longitude))
}

fn backoff(failures: u32) -> u64 {
    FIRST_BACKOFF_MS
        .saturating_mul(1 << failures.saturating_sub(1).min(10))
        .min(MAX_BACKOFF_MS)
}

/// When the timer next refreshes a site's forecast: three hours after the
/// provider last sent or confirmed it, after the timer's failures by the
/// backoff, and never before the provider's `Expires` or `Retry-After`. A
/// time later than `now` by more than those allow came from a clock that
/// ran ahead, and is due at once. With `air`, the site has a dew limit,
/// and a forecast without dew points is due 30 minutes after it arrived.
fn next_due(
    provider: ForecastProvider,
    air: bool,
    forecast: Option<&SiteForecast>,
    status: Option<&ForecastStatus>,
    now: u64,
) -> u64 {
    let forecast = forecast.filter(|f| f.provider == provider);
    let mut due = forecast.map_or(0, |f| {
        if air && !f.has_dew_points() {
            f.fetched_at_ms.saturating_add(AIR_REFRESH_MS)
        } else {
            f.checked_at_ms
                .unwrap_or(f.fetched_at_ms)
                .saturating_add(REFRESH_MS)
        }
    });
    if let Some(status) = status.filter(|s| s.failures > 0) {
        due = status
            .last_attempt_ms
            .saturating_add(backoff(status.failures));
    }
    let waits = [
        forecast.and_then(|f| f.refresh_after_ms),
        status.and_then(|s| s.retry_after_ms),
    ];
    due = waits.into_iter().flatten().fold(due, u64::max);
    if due > now.saturating_add(MAX_BACKOFF_MS.max(REFRESH_MS)) {
        return now;
    }
    due
}

/// The result of one refresh.
#[derive(Debug, PartialEq)]
pub(super) enum Refresh {
    Kept,
    NotDue,
    /// The site's provider or location changed during the request, so the
    /// response was discarded.
    Dropped,
    Failed(String),
}

/// Request the site's forecast and store the response. With `manual`, the
/// request runs before the timer's next refresh, but never sooner than the
/// provider allows, and its failure does not lengthen the timer's backoff.
pub(super) async fn refresh(
    state: &AppState,
    service: Arc<Service>,
    site: Uuid,
    manual: bool,
) -> Result<Refresh, Error> {
    // One request at a time. Fetch now does not queue behind the timer.
    let _turn = if manual {
        service.forecast_turn.try_lock().map_err(|_| {
            Error::Refused(
                "PSF Guard is already refreshing a site's forecast. Try again in a moment.".into(),
            )
        })?
    } else {
        service.forecast_turn.lock().await
    };
    let kept = service
        .clone()
        .query(move |store| Kept::read(store, site))
        .await?;
    let Some(settings) = kept.settings else {
        return Err(Error::Refused(
            "Forecasts are off for this site. Choose a provider and save the site first.".into(),
        ));
    };
    let Some(location) = kept.location else {
        return Err(Error::Refused(
            "A forecast needs the site's location. Enter its latitude and longitude, then save the site.".into(),
        ));
    };
    let now = now_ms();
    if manual {
        if let Some((after, reason)) = kept.manual_after(now) {
            return Err(Error::Refused(format!(
                "{reason} Try again in {} min.",
                (after - now).div_ceil(MINUTE_MS)
            )));
        }
    } else if now
        < next_due(
            settings.provider,
            settings.dew_limit_celsius.is_some(),
            kept.forecast.as_ref(),
            kept.status.as_ref(),
            now,
        )
    {
        return Ok(Refresh::NotDue);
    }
    // Under a dew limit, a forecast stored without the air is asked for in
    // full: "not modified" would keep it without dew points for 12 more
    // hours.
    let last_modified = kept
        .current()
        .filter(|f| settings.dew_limit_celsius.is_none() || f.has_dew_points())
        .and_then(|f| f.last_modified.clone());
    let fetched = state
        .forecast_client()
        .fetch(
            settings.provider,
            location.latitude_degrees,
            location.longitude_degrees,
            last_modified.as_deref(),
        )
        .await;
    let (latitude, longitude) = psf_guard_director_meta::site_forecast::forecast_coordinates(
        location.latitude_degrees,
        location.longitude_degrees,
    );
    let sent = (settings.provider, latitude, longitude);
    service
        .clone()
        .run(move |store| {
            let profile = store.site_profile(site)?;
            if profile.as_ref().and_then(request_key) != Some(sent) {
                return Ok(Refresh::Dropped);
            }
            let at = now_ms();
            // The provider's `Expires` holds the next request off three
            // hours at most.
            let bounded = |after: Option<u64>| after.map(|after| after.min(at + REFRESH_MS));
            let failed = |store: &mut MetaStore, error: FetchError| -> Result<Refresh, StoreError> {
                store.record_forecast_failure(
                    site,
                    &ForecastFailure {
                        at_ms: at,
                        error: error.message.clone(),
                        retry_after_ms: error.retry_after_ms,
                        from_timer: !manual,
                    },
                )?;
                Ok(Refresh::Failed(error.message))
            };
            let forecast = match fetched {
                Ok(Fetched::New { hours, .. }) if hours.is_empty() => {
                    return failed(store, "The provider's response has no cloud cover for this site.".into());
                }
                Ok(Fetched::New {
                    hours,
                    refresh_after_ms,
                    last_modified,
                }) => SiteForecast {
                    site_id: site,
                    provider: settings.provider,
                    fetched_at_ms: at,
                    checked_at_ms: None,
                    expires_at_ms: at + FORECAST_VALID_MS,
                    refresh_after_ms: bounded(refresh_after_ms),
                    last_modified,
                    hours,
                },
                // Nothing newer: the stored hours are the latest there are.
                // They keep the time they arrived; the check is noted.
                Ok(Fetched::NotModified { refresh_after_ms }) => match store.site_forecast(site)? {
                    Some(kept) if kept.provider == settings.provider && kept.fetched_at_ms <= at => SiteForecast {
                        checked_at_ms: Some(at),
                        expires_at_ms: at + FORECAST_VALID_MS,
                        refresh_after_ms: bounded(refresh_after_ms),
                        ..kept
                    },
                    _ => {
                        return failed(
                            store,
                            "The provider reported no change, but PSF Guard has no stored forecast.".into(),
                        );
                    }
                },
                Err(error) => return failed(store, error),
            };
            match store.save_site_forecast(&forecast) {
                // A newer request already stored its forecast.
                Ok(()) | Err(StoreError::Conflict) => Ok(Refresh::Kept),
                Err(StoreError::InvalidInput) => {
                    failed(store, "The provider's forecast could not be stored: it has values out of range.".into())
                }
                Err(error) => Err(error),
            }
        })
        .await
}

/// One pass of the timer: refresh each site with forecasts on that is due.
pub(super) async fn refresh_due(state: &Arc<AppState>) {
    let Ok(service) = enabled(state) else {
        return;
    };
    let sites = match service
        .clone()
        .query(|store| store.sites_with_forecasts())
        .await
    {
        Ok(sites) => sites,
        Err(error) => {
            tracing::warn!(?error, "The sites with forecasts on could not be read");
            return;
        }
    };
    for profile in sites {
        match refresh(state, service.clone(), profile.site_id, false).await {
            Ok(Refresh::Failed(error)) => tracing::warn!(
                site = %profile.site_id,
                %error,
                "A site's forecast could not be refreshed; night plans keep using the stored one until it expires"
            ),
            Err(error) => {
                tracing::warn!(site = %profile.site_id, ?error, "A site's forecast was not refreshed")
            }
            Ok(_) => {}
        }
    }
}

/// After a site's provider or location changes, request its forecast at
/// once.
pub(super) fn settings_changed(state: &Arc<AppState>, service: Arc<Service>, site: Uuid) {
    let state = Arc::clone(state);
    tokio::spawn(async move {
        match refresh(&state, service, site, false).await {
            Ok(Refresh::Failed(error)) => {
                tracing::warn!(%site, %error, "A site's first forecast could not be fetched")
            }
            Err(Error::Refused(_)) | Ok(_) => {}
            Err(error) => {
                tracing::warn!(%site, ?error, "A site's first forecast was not requested")
            }
        }
    });
}

async fn show(
    State(state): State<Arc<AppState>>,
    Path(site): Path<Uuid>,
) -> Result<Json<ApiResponse<ForecastView>>, Error> {
    let kept = enabled(&state)?
        .query(move |store| Kept::read(store, site))
        .await?;
    Ok(Json(ApiResponse::success(kept.view(now_ms()))))
}

/// Fetch now. A failed request still returns the view; its status explains
/// the error, and the stored forecast stays in use until it expires.
async fn refresh_now(
    State(state): State<Arc<AppState>>,
    Path(site): Path<Uuid>,
) -> Result<Json<ApiResponse<ForecastView>>, Error> {
    let service = enabled(&state)?;
    refresh(&state, service.clone(), site, true).await?;
    let kept = service.query(move |store| Kept::read(store, site)).await?;
    Ok(Json(ApiResponse::success(kept.view(now_ms()))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kept(
        provider: ForecastProvider,
        fetched_at_ms: u64,
        refresh_after_ms: Option<u64>,
    ) -> SiteForecast {
        SiteForecast {
            site_id: Uuid::new_v4(),
            provider,
            fetched_at_ms,
            checked_at_ms: None,
            expires_at_ms: fetched_at_ms + FORECAST_VALID_MS,
            refresh_after_ms,
            last_modified: None,
            hours: Vec::new(),
        }
    }

    fn failing(last_attempt_ms: u64, failures: u32) -> ForecastStatus {
        ForecastStatus {
            site_id: Uuid::new_v4(),
            last_attempt_ms,
            last_error: Some("down".into()),
            failures,
            retry_after_ms: None,
        }
    }

    #[test]
    fn refreshes_come_every_three_hours_back_off_and_wait_for_the_provider() {
        use ForecastProvider::*;
        let t = 100 * HOUR_MS;
        let now = t + HOUR_MS;
        assert_eq!(next_due(OpenMeteo, false, None, None, now), 0);
        let open = kept(OpenMeteo, t, None);
        assert_eq!(
            next_due(OpenMeteo, false, Some(&open), None, now),
            t + 3 * HOUR_MS
        );
        // A "not modified" check counts like a new forecast.
        let checked = SiteForecast {
            checked_at_ms: Some(t + HOUR_MS),
            ..open.clone()
        };
        assert_eq!(
            next_due(OpenMeteo, false, Some(&checked), None, now),
            t + 4 * HOUR_MS
        );
        // Under a dew limit, a forecast without dew points is due half an
        // hour after it arrived; one with them waits the three hours.
        assert_eq!(
            next_due(OpenMeteo, true, Some(&open), None, now),
            t + 30 * MINUTE_MS
        );
        let aired = SiteForecast {
            hours: vec![psf_guard_director_meta::site_forecast::ForecastHour {
                start_ms: t,
                end_ms: t + HOUR_MS,
                cloud_total: 0.0,
                cloud_low: None,
                cloud_mid: None,
                cloud_high: None,
                temperature_celsius: Some(8.0),
                dew_point_celsius: Some(2.0),
                humidity_percent: None,
            }],
            ..open.clone()
        };
        assert_eq!(
            next_due(OpenMeteo, true, Some(&aired), None, now),
            t + 3 * HOUR_MS
        );
        // A forecast from the provider the site left is due at once.
        assert_eq!(next_due(MetNorway, false, Some(&open), None, now), 0);
        // The timer's failures since: 15 min, then 30, then capped at 6 h.
        let at = t + 3 * HOUR_MS;
        assert_eq!(
            next_due(OpenMeteo, false, Some(&open), Some(&failing(at, 1)), at),
            at + 15 * MINUTE_MS
        );
        assert_eq!(
            next_due(OpenMeteo, false, Some(&open), Some(&failing(at, 2)), at),
            at + 30 * MINUTE_MS
        );
        assert_eq!(
            next_due(OpenMeteo, false, Some(&open), Some(&failing(at, 40)), at),
            at + 6 * HOUR_MS
        );
        // Never before the provider's own Expires or Retry-After.
        let met = kept(MetNorway, t, Some(t + 4 * HOUR_MS));
        assert_eq!(
            next_due(MetNorway, false, Some(&met), None, now),
            t + 4 * HOUR_MS
        );
        let told = ForecastStatus {
            retry_after_ms: Some(t + 5 * HOUR_MS),
            ..failing(at, 1)
        };
        assert_eq!(
            next_due(OpenMeteo, false, Some(&open), Some(&told), at),
            t + 5 * HOUR_MS
        );
        // A forecast stored from a clock that ran a day ahead is due now.
        let ahead = kept(OpenMeteo, t + 24 * HOUR_MS, None);
        assert_eq!(next_due(OpenMeteo, false, Some(&ahead), None, now), now);
    }

    #[test]
    fn fetch_now_waits_for_the_gap_the_provider_and_a_failure() {
        let t = 100 * HOUR_MS;
        let site = NamedIdentity {
            id: Uuid::new_v4(),
            name: "Backyard".into(),
            revision: 1,
        };
        let settings = ForecastSettings {
            provider: ForecastProvider::MetNorway,
            cloud_threshold_percent: 30,
            dew_limit_celsius: None,
        };
        let state = |forecast: Option<SiteForecast>, status: Option<ForecastStatus>| Kept {
            site: site.clone(),
            settings: Some(settings),
            location: None,
            forecast,
            status,
        };
        let fresh = kept(ForecastProvider::MetNorway, t, Some(t + 30 * MINUTE_MS));
        let (after, reason) = state(Some(fresh.clone()), None)
            .manual_after(t + MINUTE_MS)
            .unwrap();
        assert_eq!(after, t + 30 * MINUTE_MS);
        assert!(reason.starts_with("MET Norway's terms"), "{reason}");
        assert_eq!(
            state(Some(fresh), None).manual_after(t + 31 * MINUTE_MS),
            None
        );
        // After a failure, a minute's pause; a Retry-After holds it longer.
        let failed = failing(t, 0);
        assert_eq!(
            state(None, Some(failed.clone()))
                .manual_after(t + 10_000)
                .unwrap()
                .0,
            t + MINUTE_MS
        );
        let told = ForecastStatus {
            retry_after_ms: Some(t + 20 * MINUTE_MS),
            ..failed
        };
        assert!(state(None, Some(told))
            .manual_after(t + 2 * MINUTE_MS)
            .unwrap()
            .1
            .contains("asked PSF Guard to wait"));
        // A time from a clock that ran ahead is no reason to wait.
        let ahead = kept(ForecastProvider::MetNorway, t + 24 * HOUR_MS, None);
        assert_eq!(state(Some(ahead), None).manual_after(t), None);
    }

    #[test]
    fn a_new_elevation_or_a_metre_s_move_keeps_the_forecast() {
        use psf_guard_director_meta::profile::{Reported, Source};
        let profile = |latitude: f64, elevation: f64| SiteProfile {
            location: Some(Reported {
                value: Site {
                    latitude_degrees: latitude,
                    longitude_degrees: -118.3,
                    elevation_meters: elevation,
                },
                source: Source::Manual {},
                reported_at_ms: 1,
            }),
            forecast: Some(ForecastSettings {
                provider: ForecastProvider::OpenMeteo,
                cloud_threshold_percent: 30,
                dew_limit_celsius: None,
            }),
            ..SiteProfile::empty(Uuid::new_v4(), 1)
        };
        let key = request_key(&profile(34.2, 400.0));
        assert_eq!(key, request_key(&profile(34.20001, 70.0)));
        assert_ne!(key, request_key(&profile(34.3, 400.0)));
        assert_eq!(
            request_key(&SiteProfile {
                location: None,
                ..profile(34.2, 400.0)
            }),
            None
        );
    }
}
