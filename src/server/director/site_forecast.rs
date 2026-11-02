//! A site's cloud forecast (design §4.3.1 of `docs/design/night-plans.md`).
//! `GET /sites/{site}/forecast` returns the stored forecast and the result
//! of its last refresh. `POST /sites/{site}/forecast/refresh` requests a new
//! forecast at once. A timer refreshes each site with forecasts on every
//! three hours. Forecasts change night plans only: they never allow or block
//! acquisition.

use super::*;
use crate::server::forecast::Fetched;
use psf_guard_director_core::visibility::Site;
use psf_guard_director_meta::site_forecast::{
    ForecastProvider, ForecastSettings, ForecastStatus, SiteForecast, FORECAST_VALID_MS,
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

    fn view(self, now: u64) -> ForecastView {
        let next_refresh_ms = self
            .settings
            .map(|s| next_due(s.provider, self.forecast.as_ref(), self.status.as_ref()).max(now));
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

fn backoff(failures: u32) -> u64 {
    FIRST_BACKOFF_MS
        .saturating_mul(1 << failures.saturating_sub(1).min(10))
        .min(MAX_BACKOFF_MS)
}

/// When the timer next refreshes a site's forecast: three hours after the
/// last one arrived, sooner or later after a failure by the backoff, and
/// never before the provider's `Expires` time.
fn next_due(
    provider: ForecastProvider,
    forecast: Option<&SiteForecast>,
    status: Option<&ForecastStatus>,
) -> u64 {
    let forecast = forecast.filter(|f| f.provider == provider);
    let mut due = forecast.map_or(0, |f| f.fetched_at_ms.saturating_add(REFRESH_MS));
    if let Some(status) = status.filter(|s| s.failures > 0) {
        due = status
            .last_attempt_ms
            .saturating_add(backoff(status.failures));
    }
    due.max(forecast.and_then(|f| f.refresh_after_ms).unwrap_or(0))
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
/// provider allows.
pub(super) async fn refresh(
    state: &AppState,
    service: Arc<Service>,
    site: Uuid,
    manual: bool,
) -> Result<Refresh, Error> {
    let _turn = service.forecast_turn.lock().await;
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
        // After a failure a person may try again at once.
        let allowed = kept.current().map_or(0, |f| {
            (f.fetched_at_ms + MIN_GAP_MS).max(f.refresh_after_ms.unwrap_or(0))
        });
        if now < allowed {
            return Err(Error::Refused(format!(
                "The forecast was updated moments ago. Try again in {} min.",
                (allowed - now).div_ceil(MINUTE_MS)
            )));
        }
    } else if now
        < next_due(
            settings.provider,
            kept.forecast.as_ref(),
            kept.status.as_ref(),
        )
    {
        return Ok(Refresh::NotDue);
    }
    let since = kept.current().map(|f| f.fetched_at_ms);
    let fetched = state
        .forecast_client()
        .fetch(
            settings.provider,
            location.latitude_degrees,
            location.longitude_degrees,
            since,
        )
        .await;
    service
        .clone()
        .run(move |store| {
            let profile = store.site_profile(site)?;
            let unchanged = profile.as_ref().is_some_and(|p| {
                p.forecast.map(|f| f.provider) == Some(settings.provider)
                    && p.location.as_ref().map(|l| l.value) == Some(location)
            });
            if !unchanged {
                return Ok(Refresh::Dropped);
            }
            let at = now_ms();
            let failed = |store: &mut MetaStore, error: String| -> Result<Refresh, StoreError> {
                store.record_forecast_failure(site, at, &error)?;
                Ok(Refresh::Failed(error))
            };
            let forecast = match fetched {
                Ok(Fetched::New { hours, .. }) if hours.is_empty() => {
                    return failed(store, "The provider's response has no cloud cover for this site.".into());
                }
                Ok(Fetched::New {
                    hours,
                    refresh_after_ms,
                }) => SiteForecast {
                    site_id: site,
                    provider: settings.provider,
                    fetched_at_ms: at,
                    expires_at_ms: at + FORECAST_VALID_MS,
                    refresh_after_ms,
                    hours,
                },
                // Nothing newer: the kept forecast is the latest there is.
                Ok(Fetched::NotModified { refresh_after_ms }) => match store.site_forecast(site)? {
                    Some(kept) if kept.provider == settings.provider => SiteForecast {
                        fetched_at_ms: at,
                        expires_at_ms: at + FORECAST_VALID_MS,
                        refresh_after_ms,
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

async fn refresh_due(state: &Arc<AppState>) {
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
            expires_at_ms: fetched_at_ms + FORECAST_VALID_MS,
            refresh_after_ms,
            hours: Vec::new(),
        }
    }

    fn failing(last_attempt_ms: u64, failures: u32) -> ForecastStatus {
        ForecastStatus {
            site_id: Uuid::new_v4(),
            last_attempt_ms,
            last_error: Some("down".into()),
            failures,
        }
    }

    #[test]
    fn refreshes_come_every_three_hours_back_off_and_wait_for_the_provider() {
        use ForecastProvider::*;
        let t = 100 * HOUR_MS;
        assert_eq!(next_due(OpenMeteo, None, None), 0);
        let open = kept(OpenMeteo, t, None);
        assert_eq!(next_due(OpenMeteo, Some(&open), None), t + 3 * HOUR_MS);
        // A forecast from the provider the site left is due at once.
        assert_eq!(next_due(MetNorway, Some(&open), None), 0);
        // Failures since: 15 min, then 30, then capped at 6 h.
        let at = t + 3 * HOUR_MS;
        assert_eq!(
            next_due(OpenMeteo, Some(&open), Some(&failing(at, 1))),
            at + 15 * MINUTE_MS
        );
        assert_eq!(
            next_due(OpenMeteo, Some(&open), Some(&failing(at, 2))),
            at + 30 * MINUTE_MS
        );
        assert_eq!(
            next_due(OpenMeteo, Some(&open), Some(&failing(at, 40))),
            at + 6 * HOUR_MS
        );
        // Never before the provider's own Expires.
        let met = kept(MetNorway, t, Some(t + 4 * HOUR_MS));
        assert_eq!(next_due(MetNorway, Some(&met), None), t + 4 * HOUR_MS);
    }
}
