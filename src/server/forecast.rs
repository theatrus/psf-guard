//! Cloud and dew forecasts from Open-Meteo and MET Norway, for sites with forecasts
//! on. This module requests and reads forecasts. Director's site forecasts
//! decide which sites to refresh, when, and what to store.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::DateTime;
use psf_guard_director_meta::site_forecast::{
    plausible_celsius, ForecastHour, ForecastProvider, MAX_FORECAST_HOURS,
};
use reqwest::{header, StatusCode};
use serde::Deserialize;

use crate::config::ForecastsConfig;

const OPEN_METEO_URL: &str = "https://api.open-meteo.com";
const MET_NORWAY_URL: &str = "https://api.met.no";
const HOUR_MS: u64 = 3_600_000;
/// The hourly values asked of Open-Meteo.
const OPEN_METEO_HOURLY: &str = "cloud_cover,cloud_cover_low,cloud_cover_mid,cloud_cover_high,\
     temperature_2m,dew_point_2m,relative_humidity_2m";
/// The largest response read: MET Norway's full forecast is a few hundred KB.
const MAX_BODY_BYTES: usize = 4 << 20;
const MAX_REASON_CHARS: usize = 200;
const MAX_LAST_MODIFIED_CHARS: usize = 64;
/// The longest `Retry-After` honoured.
const MAX_RETRY_AFTER_MS: u64 = 6 * HOUR_MS;

/// A forecast as the provider sent it.
#[derive(Debug, PartialEq)]
pub enum Fetched {
    New {
        hours: Vec<ForecastHour>,
        /// The provider's `Expires` time: no new request before it.
        refresh_after_ms: Option<u64>,
        /// The provider's `Last-Modified`, to send back as
        /// `If-Modified-Since`.
        last_modified: Option<String>,
    },
    /// Nothing newer than the stored forecast.
    NotModified { refresh_after_ms: Option<u64> },
}

/// A failed request.
#[derive(Debug, PartialEq)]
pub struct FetchError {
    pub message: String,
    /// The provider's `Retry-After`, as an instant, at most six hours on.
    pub retry_after_ms: Option<u64>,
}

impl From<String> for FetchError {
    fn from(message: String) -> Self {
        Self {
            message,
            retry_after_ms: None,
        }
    }
}

impl From<&str> for FetchError {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// Requests forecasts from the providers. Cheap to clone. The HTTP client
/// is built on the first request, so a server whose sites take no forecast
/// never builds one.
#[derive(Clone)]
pub struct ForecastClient {
    http: Arc<OnceLock<reqwest::Client>>,
    agent: String,
    open_meteo_url: String,
    open_meteo_api_key: Option<String>,
    met_norway_url: String,
}

impl Default for ForecastClient {
    fn default() -> Self {
        Self::new(&ForecastsConfig::default())
    }
}

impl ForecastClient {
    pub fn new(config: &ForecastsConfig) -> Self {
        // MET Norway blocks requests whose User-Agent does not identify the
        // application and a way to contact it.
        let agent = match &config.contact {
            Some(contact) => format!(
                "psf-guard/{} (+https://github.com/theatrus/psf-guard; {contact})",
                env!("CARGO_PKG_VERSION")
            ),
            None => format!(
                "psf-guard/{} (+https://github.com/theatrus/psf-guard)",
                env!("CARGO_PKG_VERSION")
            ),
        };
        let base = |url: &Option<String>, default: &str| {
            url.as_deref()
                .unwrap_or(default)
                .trim_end_matches('/')
                .to_owned()
        };
        Self {
            http: Arc::default(),
            agent,
            open_meteo_url: base(&config.open_meteo_url, OPEN_METEO_URL),
            open_meteo_api_key: config.open_meteo_api_key.clone(),
            met_norway_url: base(&config.met_norway_url, MET_NORWAY_URL),
        }
    }

    fn http(&self) -> &reqwest::Client {
        self.http.get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .user_agent(self.agent.clone())
                .build()
                .expect("forecast HTTP client should build")
        })
    }

    /// The forecast at a site. Coordinates go rounded to two decimals, about
    /// a kilometre. `last_modified`, the stored forecast's `Last-Modified`,
    /// lets a provider reply that it has nothing newer.
    pub async fn fetch(
        &self,
        provider: ForecastProvider,
        latitude_degrees: f64,
        longitude_degrees: f64,
        last_modified: Option<&str>,
    ) -> Result<Fetched, FetchError> {
        let latitude = format!("{:.2}", latitude_degrees);
        let longitude = format!("{:.2}", longitude_degrees);
        match provider {
            ForecastProvider::OpenMeteo => {
                let mut query = vec![
                    ("latitude", latitude),
                    ("longitude", longitude),
                    ("hourly", OPEN_METEO_HOURLY.into()),
                    ("forecast_days", "16".into()),
                    ("timeformat", "unixtime".into()),
                    ("timezone", "GMT".into()),
                ];
                if let Some(key) = &self.open_meteo_api_key {
                    query.push(("apikey", key.clone()));
                }
                let request = self
                    .http()
                    .get(format!("{}/v1/forecast", self.open_meteo_url))
                    .query(&query);
                let (_, body) = send(request, "Open-Meteo").await?;
                let body = body.ok_or(
                    "Open-Meteo returned \"not modified\" to a request that did not ask for it.",
                )?;
                Ok(Fetched::New {
                    hours: read_open_meteo(&body)?,
                    refresh_after_ms: None,
                    last_modified: None,
                })
            }
            ForecastProvider::MetNorway => {
                let mut request = self
                    .http()
                    .get(format!(
                        "{}/weatherapi/locationforecast/2.0/complete",
                        self.met_norway_url
                    ))
                    .query(&[("lat", latitude), ("lon", longitude)]);
                // MET Norway's own `Last-Modified`, as it sent it: never a
                // time from this server's clock.
                if let Some(since) = last_modified {
                    request = request.header(header::IF_MODIFIED_SINCE, since);
                }
                let (headers, body) = send(request, "MET Norway").await?;
                let refresh_after_ms = headers
                    .get(header::EXPIRES)
                    .and_then(|value| value.to_str().ok())
                    .and_then(|value| DateTime::parse_from_rfc2822(value).ok())
                    .and_then(|at| u64::try_from(at.timestamp_millis()).ok());
                Ok(match body {
                    Some(body) => Fetched::New {
                        hours: read_met_norway(&body)?,
                        refresh_after_ms,
                        last_modified: headers
                            .get(header::LAST_MODIFIED)
                            .and_then(|value| value.to_str().ok())
                            .filter(|value| value.chars().count() <= MAX_LAST_MODIFIED_CHARS)
                            .filter(|value| !value.chars().any(char::is_control))
                            .map(str::to_owned),
                    },
                    None => Fetched::NotModified { refresh_after_ms },
                })
            }
        }
    }
}

/// The response's headers and body; no body for "not modified".
async fn send(
    request: reqwest::RequestBuilder,
    provider: &str,
) -> Result<(header::HeaderMap, Option<Vec<u8>>), FetchError> {
    let mut response = request.send().await.map_err(|error| {
        format!(
            "PSF Guard could not reach {provider}: {}",
            error.without_url()
        )
    })?;
    let status = response.status();
    let headers = response.headers().clone();
    if status == StatusCode::NOT_MODIFIED {
        return Ok((headers, None));
    }
    if response
        .content_length()
        .is_some_and(|length| length > MAX_BODY_BYTES as u64)
    {
        return Err(
            format!("{provider} sent a response larger than {MAX_BODY_BYTES} bytes.").into(),
        );
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        format!(
            "The connection to {provider} closed before the forecast arrived: {}",
            error.without_url()
        )
    })? {
        if body.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(
                format!("{provider} sent a response larger than {MAX_BODY_BYTES} bytes.").into(),
            );
        }
        body.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        // Open-Meteo explains an error in `reason`; otherwise the status is shown.
        #[derive(Deserialize)]
        struct Refusal {
            reason: String,
        }
        // Provider text reaches the log and the page: printable only.
        let reason = serde_json::from_slice::<Refusal>(&body)
            .map(|refusal| {
                refusal
                    .reason
                    .chars()
                    .filter(|c| !c.is_control())
                    .take(MAX_REASON_CHARS)
                    .collect::<String>()
            })
            .unwrap_or_else(|_| status.to_string());
        return Err(FetchError {
            message: format!("{provider} returned an error: {reason}"),
            retry_after_ms: headers
                .get(header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok())
                .and_then(|value| retry_after(value, now_ms())),
        });
    }
    Ok((headers, Some(body)))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// A `Retry-After` value, seconds or an HTTP date, as an instant no more
/// than six hours after `now_ms`.
fn retry_after(value: &str, now_ms: u64) -> Option<u64> {
    let at = match value.trim().parse::<u64>() {
        Ok(seconds) => now_ms.saturating_add(seconds.saturating_mul(1000)),
        Err(_) => u64::try_from(
            DateTime::parse_from_rfc2822(value.trim())
                .ok()?
                .timestamp_millis(),
        )
        .ok()?,
    };
    Some(at.min(now_ms.saturating_add(MAX_RETRY_AFTER_MS)))
}

/// `Sun, 06 Nov 1994 08:49:37 GMT`.
#[cfg(test)]
fn http_date(ms: u64) -> Option<String> {
    use chrono::TimeZone;
    let at = chrono::Utc
        .timestamp_millis_opt(i64::try_from(ms).ok()?)
        .single()?;
    Some(at.format("%a, %d %b %Y %H:%M:%S GMT").to_string())
}

/// A cover in percent, or `None` when the provider has no value.
fn percent(value: Option<f64>) -> Option<f64> {
    value.filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 100.0))
}

/// A temperature in °C, or `None` when the provider has none or sends one
/// no air reaches.
fn celsius(value: Option<f64>) -> Option<f64> {
    value.filter(|&v| plausible_celsius(v))
}

/// Each step lasts until the next one starts; the last as long as the one
/// before it, or an hour.
fn steps(starts: &[u64]) -> Result<Vec<(u64, u64)>, String> {
    let mut steps = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = match starts.get(i + 1) {
            Some(&next) => next,
            None => start
                .checked_add(i.checked_sub(1).map_or(HOUR_MS, |p| start - starts[p]))
                .ok_or("The forecast's hours run past any date it can hold.")?,
        };
        if end <= start {
            return Err("The forecast's hours are out of order.".into());
        }
        steps.push((start, end));
    }
    Ok(steps)
}

#[derive(Deserialize)]
struct OpenMeteo {
    hourly: OpenMeteoHourly,
}

#[derive(Deserialize)]
struct OpenMeteoHourly {
    /// Seconds since 1970, the hour each value is for.
    time: Vec<i64>,
    cloud_cover: Vec<Option<f64>>,
    #[serde(default)]
    cloud_cover_low: Vec<Option<f64>>,
    #[serde(default)]
    cloud_cover_mid: Vec<Option<f64>>,
    #[serde(default)]
    cloud_cover_high: Vec<Option<f64>>,
    #[serde(default)]
    temperature_2m: Vec<Option<f64>>,
    #[serde(default)]
    dew_point_2m: Vec<Option<f64>>,
    #[serde(default)]
    relative_humidity_2m: Vec<Option<f64>>,
}

/// Open-Meteo's hourly cover and air. Hours a model has no cloud value for
/// are left out.
fn read_open_meteo(body: &[u8]) -> Result<Vec<ForecastHour>, String> {
    let read: OpenMeteo = serde_json::from_slice(body)
        .map_err(|error| format!("Open-Meteo's response could not be read: {error}"))?;
    let hourly = read.hourly;
    if hourly.cloud_cover.len() != hourly.time.len() {
        return Err(
            "Open-Meteo's response has a different number of hours and cloud values.".into(),
        );
    }
    let starts = hourly
        .time
        .iter()
        .map(|&t| u64::try_from(t).ok().and_then(|t| t.checked_mul(1000)))
        .collect::<Option<Vec<_>>>()
        .ok_or("Open-Meteo's response has a time before 1970.")?;
    let value = |values: &[Option<f64>], i: usize| values.get(i).copied().flatten();
    let layer = |values: &[Option<f64>], i: usize| percent(value(values, i));
    Ok(steps(&starts)?
        .into_iter()
        .enumerate()
        .filter_map(|(i, (start_ms, end_ms))| {
            Some(ForecastHour {
                start_ms,
                end_ms,
                cloud_total: percent(hourly.cloud_cover[i])?,
                cloud_low: layer(&hourly.cloud_cover_low, i),
                cloud_mid: layer(&hourly.cloud_cover_mid, i),
                cloud_high: layer(&hourly.cloud_cover_high, i),
                temperature_celsius: celsius(value(&hourly.temperature_2m, i)),
                dew_point_celsius: celsius(value(&hourly.dew_point_2m, i)),
                humidity_percent: layer(&hourly.relative_humidity_2m, i),
            })
        })
        .take(MAX_FORECAST_HOURS)
        .collect())
}

#[derive(Deserialize)]
struct MetNorway {
    properties: MetProperties,
}

#[derive(Deserialize)]
struct MetProperties {
    timeseries: Vec<MetStep>,
}

#[derive(Deserialize)]
struct MetStep {
    time: String,
    data: MetData,
}

#[derive(Deserialize)]
struct MetData {
    instant: MetInstant,
}

#[derive(Deserialize)]
struct MetInstant {
    details: MetDetails,
}

#[derive(Deserialize)]
struct MetDetails {
    cloud_area_fraction: Option<f64>,
    cloud_area_fraction_low: Option<f64>,
    cloud_area_fraction_medium: Option<f64>,
    cloud_area_fraction_high: Option<f64>,
    air_temperature: Option<f64>,
    dew_point_temperature: Option<f64>,
    relative_humidity: Option<f64>,
}

/// MET Norway's cover and air: hourly, then every six hours further out, each
/// value held until the next.
fn read_met_norway(body: &[u8]) -> Result<Vec<ForecastHour>, String> {
    let read: MetNorway = serde_json::from_slice(body)
        .map_err(|error| format!("MET Norway's response could not be read: {error}"))?;
    let series = read.properties.timeseries;
    let starts = series
        .iter()
        .map(|step| {
            DateTime::parse_from_rfc3339(&step.time)
                .ok()
                .and_then(|at| u64::try_from(at.timestamp_millis()).ok())
        })
        .collect::<Option<Vec<_>>>()
        .ok_or("MET Norway's response has a time that could not be read.")?;
    Ok(steps(&starts)?
        .into_iter()
        .zip(&series)
        .filter_map(|((start_ms, end_ms), step)| {
            let details = &step.data.instant.details;
            Some(ForecastHour {
                start_ms,
                end_ms,
                cloud_total: percent(details.cloud_area_fraction)?,
                cloud_low: percent(details.cloud_area_fraction_low),
                cloud_mid: percent(details.cloud_area_fraction_medium),
                cloud_high: percent(details.cloud_area_fraction_high),
                temperature_celsius: celsius(details.air_temperature),
                dew_point_celsius: celsius(details.dew_point_temperature),
                humidity_percent: percent(details.relative_humidity),
            })
        })
        .take(MAX_FORECAST_HOURS)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Query, State},
        http::HeaderMap,
        response::IntoResponse,
        routing::get,
        Router,
    };
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    const OPEN_METEO: &str = r#"{"latitude":34.2,"longitude":-118.3,"hourly_units":{"time":"unixtime","cloud_cover":"%"},
        "hourly":{"time":[1793534400,1793538000,1793541600,1793545200],
        "cloud_cover":[0,45,null,100],"cloud_cover_low":[0,40,null,100],"cloud_cover_mid":[0,5,null,null],"cloud_cover_high":[0,0,null,20],
        "temperature_2m":[11.5,9.0,8.0,7.2],"dew_point_2m":[3.0,8.1,7.0,null],"relative_humidity_2m":[56,94,93,null]}}"#;

    const MET_NORWAY: &str = r#"{"type":"Feature","geometry":{"type":"Point","coordinates":[10.75,59.91,12]},
        "properties":{"meta":{"updated_at":"2026-11-01T11:00:00Z"},"timeseries":[
        {"time":"2026-11-01T12:00:00Z","data":{"instant":{"details":{"air_temperature":6.4,"cloud_area_fraction":12.5,"cloud_area_fraction_high":10.2,"cloud_area_fraction_low":0.0,"cloud_area_fraction_medium":3.1,"dew_point_temperature":5.1,"fog_area_fraction":0.0,"relative_humidity":91.6}}}},
        {"time":"2026-11-01T13:00:00Z","data":{"instant":{"details":{"cloud_area_fraction":100.0,"cloud_area_fraction_low":99.0}}}},
        {"time":"2026-11-01T18:00:00Z","data":{"instant":{"details":{"cloud_area_fraction":30.0}}}}]}}"#;

    #[test]
    fn open_meteo_hours_are_read_and_gaps_left_out() {
        let hours = read_open_meteo(OPEN_METEO.as_bytes()).unwrap();
        assert_eq!(hours.len(), 3);
        assert_eq!(hours[0].start_ms, 1_793_534_400_000);
        assert_eq!(hours[0].end_ms, hours[0].start_ms + HOUR_MS);
        assert_eq!(
            (hours[1].cloud_total, hours[1].cloud_low, hours[1].cloud_mid),
            (45.0, Some(40.0), Some(5.0))
        );
        // The hour with no value is skipped; the last lasts an hour.
        assert_eq!(hours[2].start_ms, 1_793_545_200_000);
        assert_eq!(hours[2].end_ms - hours[2].start_ms, HOUR_MS);
        assert_eq!(
            (hours[2].cloud_mid, hours[2].cloud_high),
            (None, Some(20.0))
        );
        assert_eq!(
            (
                hours[1].temperature_celsius,
                hours[1].dew_point_celsius,
                hours[1].humidity_percent
            ),
            (Some(9.0), Some(8.1), Some(94.0))
        );
        // An hour without a dew point keeps its temperature.
        assert_eq!(
            (hours[2].temperature_celsius, hours[2].dew_point_celsius),
            (Some(7.2), None)
        );
        // A response without the air's values still reads.
        let bare = read_open_meteo(br#"{"hourly":{"time":[3600],"cloud_cover":[5]}}"#).unwrap();
        assert_eq!(bare[0].dew_spread_celsius(), None);
        assert!(read_open_meteo(br#"{"hourly":{"time":[1,2],"cloud_cover":[1]}}"#).is_err());
        assert!(read_open_meteo(br#"{"hourly":{"time":[5,5],"cloud_cover":[1,2]}}"#).is_err());
    }

    #[test]
    fn met_norway_values_hold_until_the_next_step() {
        let hours = read_met_norway(MET_NORWAY.as_bytes()).unwrap();
        assert_eq!(hours.len(), 3);
        let noon = 1_793_534_400_000;
        assert_eq!((hours[0].start_ms, hours[0].end_ms), (noon, noon + HOUR_MS));
        assert_eq!(hours[0].cloud_mid, Some(3.1));
        assert_eq!(
            (
                hours[0].temperature_celsius,
                hours[0].dew_point_celsius,
                hours[0].humidity_percent
            ),
            (Some(6.4), Some(5.1), Some(91.6))
        );
        assert_eq!(hours[1].dew_point_celsius, None);
        // Hourly, then six-hourly: each value lasts until the next.
        assert_eq!(
            (hours[1].start_ms, hours[1].end_ms),
            (noon + HOUR_MS, noon + 6 * HOUR_MS)
        );
        assert_eq!(hours[2].end_ms - hours[2].start_ms, 5 * HOUR_MS);
        assert_eq!(hours[2].cloud_low, None);
    }

    #[test]
    fn retry_after_is_read_as_seconds_or_a_date_and_bounded() {
        let now = 1_793_534_400_000;
        assert_eq!(retry_after("120", now), Some(now + 120_000));
        let date = http_date(now + 30 * 60_000).unwrap();
        assert_eq!(date, "Sun, 01 Nov 2026 12:30:00 GMT");
        assert_eq!(retry_after(&date, now), Some(now + 30 * 60_000));
        // A provider cannot hold the site off for more than six hours.
        assert_eq!(retry_after("31536000", now), Some(now + 6 * HOUR_MS));
        assert_eq!(
            retry_after("Fri, 01 Jan 2100 00:00:00 GMT", now),
            Some(now + 6 * HOUR_MS)
        );
        assert_eq!(retry_after("soon", now), None);
        // Hours too far out to hold are refused, not wrapped.
        assert!(steps(&[u64::MAX - 10]).is_err());
    }

    #[derive(Default)]
    struct Seen {
        queries: Vec<HashMap<String, String>>,
        agents: Vec<String>,
        since: Vec<Option<String>>,
    }

    async fn provider() -> (String, Arc<Mutex<Seen>>) {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let app = Router::new()
            .route(
                "/v1/forecast",
                get(|State(seen): State<Arc<Mutex<Seen>>>, Query(query): Query<HashMap<String, String>>, headers: HeaderMap| async move {
                    let mut seen = seen.lock().unwrap();
                    seen.agents.push(headers[header::USER_AGENT].to_str().unwrap().to_owned());
                    let latitude = query.get("latitude").cloned().unwrap_or_default();
                    seen.queries.push(query);
                    if latitude == "91.00" {
                        return (StatusCode::BAD_REQUEST, r#"{"error":true,"reason":"Latitude must be in range of -90 to 90°. Given: 91.0.\u001b[31m"}"#).into_response();
                    }
                    if latitude == "1.00" {
                        let mut answer = HeaderMap::new();
                        answer.insert(header::RETRY_AFTER, "120".parse().unwrap());
                        return (StatusCode::TOO_MANY_REQUESTS, answer, "busy").into_response();
                    }
                    OPEN_METEO.into_response()
                }),
            )
            .route(
                "/weatherapi/locationforecast/2.0/complete",
                get(|State(seen): State<Arc<Mutex<Seen>>>, Query(query): Query<HashMap<String, String>>, headers: HeaderMap| async move {
                    let mut seen = seen.lock().unwrap();
                    let since = headers.get(header::IF_MODIFIED_SINCE).map(|v| v.to_str().unwrap().to_owned());
                    seen.since.push(since.clone());
                    seen.queries.push(query);
                    let mut answer = HeaderMap::new();
                    answer.insert(header::EXPIRES, "Sun, 01 Nov 2026 12:30:00 GMT".parse().unwrap());
                    answer.insert(header::LAST_MODIFIED, "Sun, 01 Nov 2026 11:00:00 GMT".parse().unwrap());
                    // Only its own Last-Modified, as sent, earns "not modified".
                    if since.as_deref() == Some("Sun, 01 Nov 2026 11:00:00 GMT") {
                        return (StatusCode::NOT_MODIFIED, answer).into_response();
                    }
                    (answer, MET_NORWAY).into_response()
                }),
            )
            .with_state(Arc::clone(&seen));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{address}/"), seen)
    }

    #[tokio::test]
    async fn requests_send_rounded_coordinates_and_identify_psf_guard() {
        let (url, seen) = provider().await;
        let client = ForecastClient::new(&ForecastsConfig {
            open_meteo_url: Some(url.clone()),
            open_meteo_api_key: Some("paid".into()),
            met_norway_url: Some(url),
            contact: Some("ops@example.org".into()),
        });
        let fetched = client
            .fetch(ForecastProvider::OpenMeteo, 34.21789, -118.31234, None)
            .await
            .unwrap();
        assert!(
            matches!(&fetched, Fetched::New { hours, refresh_after_ms: None, last_modified: None } if hours.len() == 3)
        );
        // The provider's reason, printable only.
        let refused = client
            .fetch(ForecastProvider::OpenMeteo, 91.0, 0.0, None)
            .await
            .unwrap_err();
        assert_eq!(
            refused.message,
            "Open-Meteo returned an error: Latitude must be in range of -90 to 90°. Given: 91.0.[31m"
        );
        let busy = client
            .fetch(ForecastProvider::OpenMeteo, 1.0, 0.0, None)
            .await
            .unwrap_err();
        assert_eq!(
            busy.message,
            "Open-Meteo returned an error: 429 Too Many Requests"
        );
        let wait = busy.retry_after_ms.unwrap() - now_ms();
        assert!((100_000..=120_000).contains(&wait), "{wait}");

        let expires = 1_793_536_200_000;
        let fetched = client
            .fetch(ForecastProvider::MetNorway, 59.91, 10.75, None)
            .await
            .unwrap();
        let Fetched::New {
            hours,
            refresh_after_ms,
            last_modified,
        } = &fetched
        else {
            panic!("{fetched:?}");
        };
        assert_eq!((hours.len(), *refresh_after_ms), (3, Some(expires)));
        assert_eq!(
            last_modified.as_deref(),
            Some("Sun, 01 Nov 2026 11:00:00 GMT")
        );
        let again = client
            .fetch(
                ForecastProvider::MetNorway,
                59.91,
                10.75,
                last_modified.as_deref(),
            )
            .await
            .unwrap();
        assert_eq!(
            again,
            Fetched::NotModified {
                refresh_after_ms: Some(expires)
            }
        );

        let seen = seen.lock().unwrap();
        assert_eq!(seen.queries[0]["latitude"], "34.22");
        assert_eq!(seen.queries[0]["longitude"], "-118.31");
        assert_eq!(seen.queries[0]["apikey"], "paid");
        assert_eq!(
            seen.queries[0]["hourly"],
            "cloud_cover,cloud_cover_low,cloud_cover_mid,cloud_cover_high,\
             temperature_2m,dew_point_2m,relative_humidity_2m"
        );
        assert_eq!(
            (
                seen.queries[3]["lat"].as_str(),
                seen.queries[3]["lon"].as_str()
            ),
            ("59.91", "10.75")
        );
        assert!(
            seen.agents[0].starts_with("psf-guard/")
                && seen.agents[0].ends_with("; ops@example.org)")
        );
        assert_eq!(
            seen.since,
            vec![None, Some("Sun, 01 Nov 2026 11:00:00 GMT".to_owned())]
        );
    }

    #[tokio::test]
    async fn an_unreachable_provider_is_an_error() {
        let client = ForecastClient::new(&ForecastsConfig {
            open_meteo_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        });
        let error = client
            .fetch(ForecastProvider::OpenMeteo, 0.0, 0.0, None)
            .await
            .unwrap_err();
        assert!(
            error
                .message
                .starts_with("PSF Guard could not reach Open-Meteo"),
            "{error:?}"
        );
    }
}
