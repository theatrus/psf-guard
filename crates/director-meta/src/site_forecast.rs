//! A site's cloud forecast: which provider the site takes it from, the
//! thresholds under which an hour counts as clear, and the latest forecast
//! fetched. Planning input only, like a manual mark: a forecast never allows
//! or blocks acquisition.
//!
//! Windows are worked out when the forecast is read, from the threshold the
//! site has then, so a changed threshold applies at once.

use super::*;
use psf_guard_director_core::windows::Interval;
use std::str::FromStr;

/// Most hours a stored forecast may hold: 16 days.
pub const MAX_FORECAST_HOURS: usize = 16 * 24;
/// How long a forecast is good for after it was fetched.
pub const FORECAST_VALID_MS: u64 = 12 * 3_600_000;
/// The threshold a site starts with: an hour at or under this much total
/// cloud cover counts as clear.
pub const DEFAULT_CLOUD_THRESHOLD_PERCENT: u8 = 30;
/// The largest dew limit a site may set, in °C above the dew point.
pub const MAX_DEW_LIMIT_CELSIUS: u8 = 20;

const MAX_TIME_MS: u64 = 4_102_444_800_000; // 2100-01-01
const MAX_ERROR_CHARS: usize = 300;
const MAX_LAST_MODIFIED_CHARS: usize = 64;
/// A stored forecast newer than a replacement by less than this came from a
/// refresh that raced it, and stays. One newer by more came from a clock
/// that ran ahead, and is replaced.
const RACE_MS: i64 = 3_600_000;

/// Where a site's forecast comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForecastProvider {
    /// Open-Meteo: hourly, up to 16 days, free for non-commercial use.
    OpenMeteo,
    /// MET Norway's Locationforecast: about 9 days, free, commercial use
    /// included.
    MetNorway,
}

impl ForecastProvider {
    fn as_str(self) -> &'static str {
        match self {
            Self::OpenMeteo => "open_meteo",
            Self::MetNorway => "met_norway",
        }
    }

    /// The provider's name, as people know it.
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenMeteo => "Open-Meteo",
            Self::MetNorway => "MET Norway",
        }
    }

    /// The credit the provider's licence requires wherever its data appears.
    pub fn attribution(self) -> &'static str {
        match self {
            Self::OpenMeteo => "Weather data by Open-Meteo.com (CC BY 4.0)",
            Self::MetNorway => "Weather data from MET Norway (CC BY 4.0)",
        }
    }
}

impl fmt::Display for ForecastProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ForecastProvider {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            "open_meteo" => Ok(Self::OpenMeteo),
            "met_norway" => Ok(Self::MetNorway),
            _ => Err(Error::InvalidInput),
        }
    }
}

/// A site's forecast provider and clear-sky thresholds. Without settings,
/// forecasts are off for the site and PSF Guard sends nothing about it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastSettings {
    pub provider: ForecastProvider,
    /// An hour at or under this much total cloud cover, in percent, counts
    /// as clear.
    pub cloud_threshold_percent: u8,
    /// An hour whose air is this close to its dew point or closer, in °C,
    /// does not count as clear. `None` leaves dew out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dew_limit_celsius: Option<u8>,
}

impl ForecastSettings {
    pub fn validate(&self) -> Result<(), Error> {
        if self.cloud_threshold_percent > 100
            || self
                .dew_limit_celsius
                .is_some_and(|limit| limit > MAX_DEW_LIMIT_CELSIUS)
        {
            return Err(Error::InvalidInput);
        }
        Ok(())
    }

    /// Whether an hour counts as clear: cloud at or under the threshold,
    /// and, with a dew limit, air more than the limit above its dew point.
    /// An hour without a dew point is judged by its cloud alone.
    pub fn clear(&self, hour: &ForecastHour) -> bool {
        hour.cloud_total <= f64::from(self.cloud_threshold_percent)
            && !self
                .dew_limit_celsius
                .zip(hour.dew_spread_celsius())
                .is_some_and(|(limit, spread)| spread <= f64::from(limit))
    }
}

/// One step of a forecast: cloud cover, in percent, and the air at 2 m,
/// from `start_ms` to `end_ms` (an hour, or six hours further out for some
/// providers).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastHour {
    pub start_ms: u64,
    pub end_ms: u64,
    pub cloud_total: f64,
    #[serde(default)]
    pub cloud_low: Option<f64>,
    #[serde(default)]
    pub cloud_mid: Option<f64>,
    #[serde(default)]
    pub cloud_high: Option<f64>,
    /// Air temperature, °C.
    #[serde(default)]
    pub temperature_celsius: Option<f64>,
    /// Dew point, °C.
    #[serde(default)]
    pub dew_point_celsius: Option<f64>,
    /// Relative humidity, percent.
    #[serde(default)]
    pub humidity_percent: Option<f64>,
}

impl ForecastHour {
    /// How far the air is above its dew point, in °C; never below zero.
    pub fn dew_spread_celsius(&self) -> Option<f64> {
        let (air, dew) = self.temperature_celsius.zip(self.dew_point_celsius)?;
        Some((air - dew).max(0.0))
    }
}

/// A site's latest forecast.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteForecast {
    pub site_id: Uuid,
    pub provider: ForecastProvider,
    /// When these hours arrived.
    pub fetched_at_ms: u64,
    /// When the provider last confirmed them, by answering "not modified";
    /// `None` until it has.
    #[serde(default)]
    pub checked_at_ms: Option<u64>,
    /// After this the forecast counts as no information.
    pub expires_at_ms: u64,
    /// No new request before this: the provider's `Expires`.
    #[serde(default)]
    pub refresh_after_ms: Option<u64>,
    /// The provider's `Last-Modified`, sent back as `If-Modified-Since`.
    #[serde(default)]
    pub last_modified: Option<String>,
    /// In time order, apart.
    pub hours: Vec<ForecastHour>,
}

/// How a site's refresh went: the last attempt and, after a failure, why.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForecastStatus {
    pub site_id: Uuid,
    pub last_attempt_ms: u64,
    /// `None` when the last attempt succeeded.
    pub last_error: Option<String>,
    /// The timer's failures in a row, for its backoff. A failed Fetch now
    /// is not counted.
    pub failures: u32,
    /// No new request before this: the provider's `Retry-After` on the last
    /// failure.
    #[serde(default)]
    pub retry_after_ms: Option<u64>,
}

/// A failed request, to record.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ForecastFailure {
    pub at_ms: u64,
    pub error: String,
    /// The provider's `Retry-After`, as an instant.
    pub retry_after_ms: Option<u64>,
    /// Whether the timer made the request, so the failure counts toward its
    /// backoff.
    pub from_timer: bool,
}

/// What a forecast says about one night.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ForecastNight {
    /// The night's span the forecast covers; outside it there is no
    /// information.
    pub covered: Vec<Interval>,
    /// The clear hours inside `covered`.
    pub clear: Vec<Interval>,
    /// The forecast's steps inside the night, for drawing.
    pub hours: Vec<ForecastHour>,
}

impl SiteForecast {
    pub fn validate(&self) -> Result<(), Error> {
        valid_id(self.site_id)?;
        if self.fetched_at_ms > MAX_TIME_MS
            || self.expires_at_ms > MAX_TIME_MS
            || self.expires_at_ms < self.fetched_at_ms
            || self.refresh_after_ms.is_some_and(|at| at > MAX_TIME_MS)
            || self
                .checked_at_ms
                .is_some_and(|at| at > MAX_TIME_MS || at < self.fetched_at_ms)
            || self.last_modified.as_ref().is_some_and(|text| {
                text.chars().count() > MAX_LAST_MODIFIED_CHARS || text.chars().any(char::is_control)
            })
            || self.hours.len() > MAX_FORECAST_HOURS
        {
            return Err(Error::InvalidInput);
        }
        let mut last_end = 0;
        for hour in &self.hours {
            let percent = |v: f64| v.is_finite() && (0.0..=100.0).contains(&v);
            if hour.start_ms >= hour.end_ms
                || hour.end_ms > MAX_TIME_MS
                || hour.start_ms < last_end
                || !percent(hour.cloud_total)
                || [
                    hour.cloud_low,
                    hour.cloud_mid,
                    hour.cloud_high,
                    hour.humidity_percent,
                ]
                .into_iter()
                .flatten()
                .any(|v| !percent(v))
                || [hour.temperature_celsius, hour.dew_point_celsius]
                    .into_iter()
                    .flatten()
                    .any(|v| !v.is_finite() || !(-100.0..=70.0).contains(&v))
            {
                return Err(Error::InvalidInput);
            }
            last_end = hour.end_ms;
        }
        Ok(())
    }

    /// Whether the forecast still counts at `now_ms`.
    pub fn fresh(&self, now_ms: u64) -> bool {
        now_ms < self.expires_at_ms
    }

    /// What the forecast says about the night spanning `span`: the hours it
    /// covers there, and those the site's settings count as clear. `None`
    /// once it has expired, or when it covers none of the night.
    pub fn night(
        &self,
        span: Interval,
        settings: &ForecastSettings,
        now_ms: u64,
    ) -> Option<ForecastNight> {
        if !self.fresh(now_ms) {
            return None;
        }
        let mut covered: Vec<Interval> = Vec::new();
        let mut clear: Vec<Interval> = Vec::new();
        let mut hours = Vec::new();
        let push = |list: &mut Vec<Interval>, start_ms: u64, end_ms: u64| match list.last_mut() {
            Some(last) if last.end_ms == start_ms => last.end_ms = end_ms,
            _ => list.push(Interval { start_ms, end_ms }),
        };
        for hour in &self.hours {
            let start_ms = hour.start_ms.max(span.start_ms);
            let end_ms = hour.end_ms.min(span.end_ms);
            if start_ms >= end_ms {
                continue;
            }
            hours.push(*hour);
            push(&mut covered, start_ms, end_ms);
            if settings.clear(hour) {
                push(&mut clear, start_ms, end_ms);
            }
        }
        (!covered.is_empty()).then_some(ForecastNight {
            covered,
            clear,
            hours,
        })
    }
}

pub(crate) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS site_forecast(
            site_id TEXT PRIMARY KEY NOT NULL REFERENCES site(id),
            provider TEXT NOT NULL,
            fetched_at_ms INTEGER NOT NULL,
            expires_at_ms INTEGER NOT NULL,
            payload TEXT NOT NULL);
         CREATE TABLE IF NOT EXISTS site_forecast_status(
            site_id TEXT PRIMARY KEY NOT NULL REFERENCES site(id),
            payload TEXT NOT NULL);",
    )?;
    Ok(())
}

pub(crate) fn validate_tables(conn: &Connection) -> Result<(), Error> {
    for sql in [
        "SELECT site_id,provider,fetched_at_ms,expires_at_ms,payload FROM site_forecast LIMIT 0",
        "SELECT site_id,payload FROM site_forecast_status LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
    }
    Ok(())
}

impl MetaStore {
    /// A site's latest forecast, fresh or not.
    pub fn site_forecast(&self, site: Uuid) -> Result<Option<SiteForecast>, Error> {
        valid_id(site)?;
        let payload: Option<String> = self
            .connection
            .query_row(
                "SELECT payload FROM site_forecast WHERE site_id=?1",
                [site.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        payload
            .map(|payload| {
                let value: SiteForecast = super::configuration::decode(payload.into_bytes())?;
                value.validate().map_err(|_| Error::CorruptDatabase)?;
                if value.site_id != site {
                    return Err(Error::CorruptDatabase);
                }
                Ok(value)
            })
            .transpose()
    }

    /// Replace a site's forecast with a newer one, and note the success.
    /// An older forecast than the one stored is refused with `Conflict`, so
    /// two refreshes racing cannot put an old one back. A stored forecast
    /// more than an hour newer came from a clock that ran ahead, and is
    /// replaced.
    pub fn save_site_forecast(&mut self, forecast: &SiteForecast) -> Result<(), Error> {
        forecast.validate()?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Site, forecast.site_id)?.is_none() {
            return Err(Error::NotFound);
        }
        let stored: Option<i64> = tx
            .query_row(
                "SELECT fetched_at_ms FROM site_forecast WHERE site_id=?1",
                [forecast.site_id.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let fetched = i64::try_from(forecast.fetched_at_ms).unwrap_or(i64::MAX);
        if stored.is_some_and(|at| at > fetched && at - fetched < RACE_MS) {
            return Err(Error::Conflict);
        }
        let payload = super::configuration::encode(forecast)?;
        tx.execute(
            "INSERT INTO site_forecast(site_id,provider,fetched_at_ms,expires_at_ms,payload) VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(site_id) DO UPDATE SET provider=excluded.provider, fetched_at_ms=excluded.fetched_at_ms,
             expires_at_ms=excluded.expires_at_ms, payload=excluded.payload",
            params![
                forecast.site_id.to_string(),
                forecast.provider.as_str(),
                i64::try_from(forecast.fetched_at_ms).map_err(|_| Error::InvalidInput)?,
                i64::try_from(forecast.expires_at_ms).map_err(|_| Error::InvalidInput)?,
                payload
            ],
        )?;
        write_status(
            &tx,
            &ForecastStatus {
                site_id: forecast.site_id,
                last_attempt_ms: forecast.checked_at_ms.unwrap_or(forecast.fetched_at_ms),
                last_error: None,
                failures: 0,
                retry_after_ms: None,
            },
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Note a failed refresh. The stored forecast stays, with its expiry.
    pub fn record_forecast_failure(
        &mut self,
        site: Uuid,
        failure: &ForecastFailure,
    ) -> Result<ForecastStatus, Error> {
        valid_id(site)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if read_named(&tx, Kind::Site, site)?.is_none() {
            return Err(Error::NotFound);
        }
        let failures = read_status(&tx, site)?.map_or(0, |status| status.failures);
        let message: String = failure
            .error
            .chars()
            .filter(|c| !c.is_control())
            .take(MAX_ERROR_CHARS)
            .collect();
        let status = ForecastStatus {
            site_id: site,
            last_attempt_ms: failure.at_ms.min(MAX_TIME_MS),
            last_error: Some(message),
            failures: if failure.from_timer {
                failures.saturating_add(1)
            } else {
                failures
            },
            retry_after_ms: failure.retry_after_ms.map(|at| at.min(MAX_TIME_MS)),
        };
        write_status(&tx, &status)?;
        tx.commit()?;
        Ok(status)
    }

    /// How the site's last refresh went, if it has had one.
    pub fn forecast_status(&self, site: Uuid) -> Result<Option<ForecastStatus>, Error> {
        valid_id(site)?;
        read_status(&self.connection, site)
    }

    /// The sites with forecasts on, with their profiles.
    pub fn sites_with_forecasts(&self) -> Result<Vec<super::site_profile::SiteProfile>, Error> {
        let mut statement = self
            .connection
            .prepare("SELECT site_id FROM site_profile ORDER BY site_id")?;
        let ids = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        let mut sites = Vec::new();
        for id in ids {
            let id = Uuid::parse_str(&id).map_err(|_| Error::CorruptDatabase)?;
            if let Some(profile) = self.site_profile(id)?
                && profile.forecast.is_some()
            {
                sites.push(profile);
            }
        }
        Ok(sites)
    }

    /// Forget a site's forecast and its status, as when it stops taking one.
    pub fn clear_site_forecast(&mut self, site: Uuid) -> Result<(), Error> {
        valid_id(site)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM site_forecast WHERE site_id=?1",
            [site.to_string()],
        )?;
        tx.execute(
            "DELETE FROM site_forecast_status WHERE site_id=?1",
            [site.to_string()],
        )?;
        tx.commit()?;
        Ok(())
    }
}

fn read_status(conn: &Connection, site: Uuid) -> Result<Option<ForecastStatus>, Error> {
    let payload: Option<String> = conn
        .query_row(
            "SELECT payload FROM site_forecast_status WHERE site_id=?1",
            [site.to_string()],
            |row| row.get(0),
        )
        .optional()?;
    payload
        .map(|payload| {
            let value: ForecastStatus = super::configuration::decode(payload.into_bytes())?;
            if value.site_id != site {
                return Err(Error::CorruptDatabase);
            }
            Ok(value)
        })
        .transpose()
}

fn write_status(conn: &Connection, status: &ForecastStatus) -> Result<(), Error> {
    let payload = super::configuration::encode(status)?;
    conn.execute(
        "INSERT INTO site_forecast_status(site_id,payload) VALUES(?1,?2)
         ON CONFLICT(site_id) DO UPDATE SET payload=excluded.payload",
        params![status.site_id.to_string(), payload],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOUR: u64 = 3_600_000;

    fn hours(start: u64, covers: &[f64]) -> Vec<ForecastHour> {
        covers
            .iter()
            .enumerate()
            .map(|(i, cover)| ForecastHour {
                start_ms: start + i as u64 * HOUR,
                end_ms: start + (i as u64 + 1) * HOUR,
                cloud_total: *cover,
                cloud_low: None,
                cloud_mid: None,
                cloud_high: None,
                temperature_celsius: None,
                dew_point_celsius: None,
                humidity_percent: None,
            })
            .collect()
    }

    fn clouds(threshold: u8) -> ForecastSettings {
        ForecastSettings {
            provider: ForecastProvider::OpenMeteo,
            cloud_threshold_percent: threshold,
            dew_limit_celsius: None,
        }
    }

    #[test]
    fn clear_hours_under_the_threshold_become_windows_inside_the_night() {
        let span = Interval {
            start_ms: 100 * HOUR,
            end_ms: 124 * HOUR,
        };
        let forecast = SiteForecast {
            site_id: Uuid::new_v4(),
            provider: ForecastProvider::OpenMeteo,
            fetched_at_ms: 90 * HOUR,
            checked_at_ms: None,
            expires_at_ms: 102 * HOUR,
            refresh_after_ms: None,
            last_modified: None,
            // From two hours before the night to eight hours into it.
            hours: hours(
                98 * HOUR,
                &[0.0, 0.0, 80.0, 20.0, 30.0, 31.0, 10.0, 100.0, 0.0, 0.0],
            ),
        };
        forecast.validate().unwrap();
        let night = forecast.night(span, &clouds(30), 95 * HOUR).unwrap();
        assert_eq!(
            night.covered,
            vec![Interval {
                start_ms: 100 * HOUR,
                end_ms: 108 * HOUR
            }]
        );
        assert_eq!(
            night.clear,
            vec![
                Interval {
                    start_ms: 101 * HOUR,
                    end_ms: 103 * HOUR
                },
                Interval {
                    start_ms: 104 * HOUR,
                    end_ms: 105 * HOUR
                },
                Interval {
                    start_ms: 106 * HOUR,
                    end_ms: 108 * HOUR
                },
            ]
        );
        assert_eq!(night.hours.len(), 8);
        // A stricter threshold leaves less; an expired forecast says nothing.
        assert_eq!(
            forecast
                .night(span, &clouds(10), 95 * HOUR)
                .unwrap()
                .clear
                .len(),
            2
        );
        assert_eq!(forecast.night(span, &clouds(30), 102 * HOUR), None);
        // A night past the forecast's range has no information.
        let later = Interval {
            start_ms: 124 * HOUR,
            end_ms: 148 * HOUR,
        };
        assert_eq!(forecast.night(later, &clouds(30), 95 * HOUR), None);
    }

    #[test]
    fn hours_near_the_dew_point_are_not_clear_under_a_dew_limit() {
        let span = Interval {
            start_ms: 100 * HOUR,
            end_ms: 104 * HOUR,
        };
        let mut steps = hours(100 * HOUR, &[0.0, 0.0, 0.0, 0.0]);
        // 10 °C above the dew point, then 3, then exactly 2, then below
        // it; the last has no dew point at all.
        for (hour, (air, dew)) in
            steps
                .iter_mut()
                .zip([(12.0, 2.0), (8.0, 5.0), (6.0, 4.0), (3.0, 3.5)])
        {
            hour.temperature_celsius = Some(air);
            hour.dew_point_celsius = Some(dew);
            hour.humidity_percent = Some(90.0);
        }
        steps[3].dew_point_celsius = None;
        let forecast = SiteForecast {
            site_id: Uuid::new_v4(),
            provider: ForecastProvider::OpenMeteo,
            fetched_at_ms: 90 * HOUR,
            checked_at_ms: None,
            expires_at_ms: 110 * HOUR,
            refresh_after_ms: None,
            last_modified: None,
            hours: steps,
        };
        forecast.validate().unwrap();
        assert_eq!(forecast.hours[1].dew_spread_celsius(), Some(3.0));
        assert_eq!(forecast.hours[3].dew_spread_celsius(), None);
        let dew = ForecastSettings {
            dew_limit_celsius: Some(2),
            ..clouds(30)
        };
        let night = forecast.night(span, &dew, 95 * HOUR).unwrap();
        assert_eq!(
            night.clear,
            vec![
                Interval {
                    start_ms: 100 * HOUR,
                    end_ms: 102 * HOUR
                },
                Interval {
                    start_ms: 103 * HOUR,
                    end_ms: 104 * HOUR
                },
            ]
        );
        // Without a limit, dew leaves every hour clear.
        let night = forecast.night(span, &clouds(30), 95 * HOUR).unwrap();
        assert_eq!(night.clear, vec![span]);
        // A dew point that is not a number is refused.
        let mut wild = forecast.clone();
        wild.hours[0].dew_point_celsius = Some(f64::NAN);
        assert!(wild.validate().is_err());
    }

    #[test]
    fn forecasts_round_trip_newest_wins_and_a_failure_keeps_the_last() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
        let site = Uuid::new_v4();
        store.create_site(site, "Backyard").unwrap();
        assert_eq!(store.site_forecast(site).unwrap(), None);
        let first = SiteForecast {
            site_id: site,
            provider: ForecastProvider::MetNorway,
            fetched_at_ms: 10 * HOUR,
            checked_at_ms: None,
            expires_at_ms: 22 * HOUR,
            refresh_after_ms: Some(11 * HOUR),
            last_modified: Some("Sun, 01 Nov 2026 10:00:00 GMT".into()),
            hours: hours(10 * HOUR, &[5.0, 50.0]),
        };
        store.save_site_forecast(&first).unwrap();
        assert_eq!(store.site_forecast(site).unwrap(), Some(first.clone()));
        assert_eq!(store.forecast_status(site).unwrap().unwrap().failures, 0);
        let failure = |at_ms: u64, error: &str, from_timer: bool| ForecastFailure {
            at_ms,
            error: error.into(),
            retry_after_ms: None,
            from_timer,
        };
        let failed = store
            .record_forecast_failure(site, &failure(13 * HOUR, "timed out\nafter 12 s", true))
            .unwrap();
        assert_eq!(
            (failed.failures, failed.last_error.as_deref()),
            (1, Some("timed outafter 12 s"))
        );
        assert_eq!(
            store
                .record_forecast_failure(site, &failure(14 * HOUR, "again", true))
                .unwrap()
                .failures,
            2
        );
        // A failed Fetch now is noted, but does not lengthen the backoff.
        let manual = ForecastFailure {
            retry_after_ms: Some(15 * HOUR),
            ..failure(14 * HOUR, "429", false)
        };
        let noted = store.record_forecast_failure(site, &manual).unwrap();
        assert_eq!((noted.failures, noted.retry_after_ms), (2, Some(15 * HOUR)));
        // The failure left the forecast as it was.
        assert_eq!(store.site_forecast(site).unwrap(), Some(first.clone()));
        // An older forecast cannot replace a newer one.
        let older = SiteForecast {
            fetched_at_ms: 9 * HOUR + HOUR / 2,
            ..first.clone()
        };
        assert!(matches!(
            store.save_site_forecast(&older),
            Err(Error::Conflict)
        ));
        // One stored from a clock that ran hours ahead is replaced.
        let ahead = SiteForecast {
            fetched_at_ms: 40 * HOUR,
            expires_at_ms: 52 * HOUR,
            ..first.clone()
        };
        store.save_site_forecast(&ahead).unwrap();
        store.save_site_forecast(&first).unwrap();
        assert_eq!(store.site_forecast(site).unwrap(), Some(first.clone()));
        let newer = SiteForecast {
            fetched_at_ms: 15 * HOUR,
            expires_at_ms: 27 * HOUR,
            ..first
        };
        store.save_site_forecast(&newer).unwrap();
        assert_eq!(store.forecast_status(site).unwrap().unwrap().failures, 0);
        store.clear_site_forecast(site).unwrap();
        assert_eq!(store.site_forecast(site).unwrap(), None);
        // Bad forecasts are refused.
        let backwards = SiteForecast {
            expires_at_ms: 1,
            ..newer.clone()
        };
        assert!(matches!(
            store.save_site_forecast(&backwards),
            Err(Error::InvalidInput)
        ));
        let overcast = SiteForecast {
            hours: hours(10 * HOUR, &[101.0]),
            ..newer
        };
        assert!(matches!(
            store.save_site_forecast(&overcast),
            Err(Error::InvalidInput)
        ));
        assert_eq!(
            "met_norway".parse::<ForecastProvider>().unwrap(),
            ForecastProvider::MetNorway
        );
        assert!("dark_sky".parse::<ForecastProvider>().is_err());
    }

    #[test]
    fn only_sites_that_opted_in_are_listed() {
        use crate::site_profile::SiteProfile;
        let dir = tempfile::tempdir().unwrap();
        let mut store = MetaStore::create(&dir.path().join("meta.sqlite")).unwrap();
        let (home, club) = (Uuid::new_v4(), Uuid::new_v4());
        store.create_site(home, "Home").unwrap();
        store.create_site(club, "Club").unwrap();
        store
            .save_site_profile(&SiteProfile::empty(home, 1), 0)
            .unwrap();
        let settings = ForecastSettings {
            provider: ForecastProvider::OpenMeteo,
            cloud_threshold_percent: DEFAULT_CLOUD_THRESHOLD_PERCENT,
            dew_limit_celsius: None,
        };
        let opted = SiteProfile {
            forecast: Some(settings),
            ..SiteProfile::empty(club, 1)
        };
        store.save_site_profile(&opted, 0).unwrap();
        let listed = store.sites_with_forecasts().unwrap();
        assert_eq!(
            listed.iter().map(|p| p.site_id).collect::<Vec<_>>(),
            vec![club]
        );
        let strict = ForecastSettings {
            cloud_threshold_percent: 101,
            ..settings
        };
        let refused = SiteProfile {
            forecast: Some(strict),
            ..SiteProfile::empty(home, 2)
        };
        assert!(matches!(
            store.save_site_profile(&refused, 1),
            Err(Error::InvalidInput)
        ));
        let dewy = SiteProfile {
            forecast: Some(ForecastSettings {
                dew_limit_celsius: Some(MAX_DEW_LIMIT_CELSIUS + 1),
                ..settings
            }),
            ..SiteProfile::empty(home, 2)
        };
        assert!(matches!(
            store.save_site_profile(&dewy, 1),
            Err(Error::InvalidInput)
        ));
    }
}
