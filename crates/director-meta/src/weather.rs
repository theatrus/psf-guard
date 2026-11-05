//! Rig-reported weather and short-lived cloud decisions. Reports never replace
//! a safety monitor. Forecasts retain their provider and prediction interval.
use super::*;
use crate::site_forecast::{
    forecast_coordinates, ForecastHour, ForecastProvider, DEFAULT_CLOUD_THRESHOLD_PERCENT,
};

pub const WEATHER_MAX_AGE_MS: u64 = 90_000;
pub const CLOUD_DECISION_MS: u64 = 30_000;
const RETENTION_MS: u64 = 30 * 24 * 3_600_000;
const MAX_TIME_MS: u64 = 4_102_444_800_000;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherValues {
    pub cloud_percent: Option<f64>,
    pub temperature_celsius: Option<f64>,
    pub dew_point_celsius: Option<f64>,
    pub humidity_percent: Option<f64>,
    pub pressure_hpa: Option<f64>,
    pub rain_mm_per_hour: Option<f64>,
    pub sky_brightness_lux: Option<f64>,
    pub sky_quality_mag_arcsec2: Option<f64>,
    pub sky_temperature_celsius: Option<f64>,
    pub seeing_arcsec: Option<f64>,
    pub wind_direction_degrees: Option<f64>,
    pub wind_speed_m_s: Option<f64>,
    pub wind_gust_m_s: Option<f64>,
}

impl WeatherValues {
    fn validate(&self) -> Result<(), Error> {
        if [self.temperature_celsius, self.dew_point_celsius]
            .into_iter()
            .flatten()
            .any(|v| !crate::site_forecast::plausible_celsius(v))
        {
            return Err(Error::InvalidInput);
        }
        for (value, low, high) in [
            (self.cloud_percent, 0., 100.),
            (self.temperature_celsius, -150., 150.),
            (self.dew_point_celsius, -150., 150.),
            (self.humidity_percent, 0., 100.),
            (self.pressure_hpa, 0., 1500.),
            (self.rain_mm_per_hour, 0., 10_000.),
            (self.sky_brightness_lux, 0., 1_000_000.),
            (self.sky_quality_mag_arcsec2, 0., 40.),
            (self.sky_temperature_celsius, -273.15, 150.),
            (self.seeing_arcsec, 0., 100.),
            (self.wind_direction_degrees, 0., 360.),
            (self.wind_speed_m_s, 0., 200.),
            (self.wind_gust_m_s, 0., 200.),
        ] {
            if value.is_some_and(|v| !v.is_finite() || !(low..=high).contains(&v)) {
                return Err(Error::InvalidInput);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherSample {
    pub sample_id: Uuid,
    pub profile_id: Uuid,
    /// When NINA delivered these values, not a claimed sensor observation time.
    pub reported_at_ms: u64,
    pub source_name: String,
    /// SHA-256 of NINA's device identity; no connection string or API key.
    pub source_key: String,
    pub connected: bool,
    pub values: WeatherValues,
}

impl WeatherSample {
    pub fn validate(&self, now: u64) -> Result<(), Error> {
        valid_id(self.sample_id)?;
        valid_id(self.profile_id)?;
        if now > MAX_TIME_MS
            || self.reported_at_ms == 0
            || self.reported_at_ms > now.saturating_add(5_000)
            || self.source_name.is_empty()
            || self.source_name.chars().count() > 128
            || self.source_name.chars().any(char::is_control)
            || self.source_key.len() != 64
            || !self
                .source_key
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::InvalidInput);
        }
        self.values.validate()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WeatherObservation {
    pub rig_id: Uuid,
    pub sample: WeatherSample,
    pub received_at_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CloudState {
    Clear,
    Cloudy,
    Dewy,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CloudAssessment {
    pub state: CloudState,
    pub reason: &'static str,
    /// Lifetime from response generation, reduced by the client round trip.
    pub valid_for_ms: u64,
}

impl CloudAssessment {
    fn unknown(reason: &'static str) -> Self {
        Self {
            state: CloudState::Unknown,
            reason,
            valid_for_ms: 0,
        }
    }
    fn assess(cloud: f64, threshold: u8, remaining: u64) -> Self {
        Self {
            state: if cloud <= f64::from(threshold) {
                CloudState::Clear
            } else {
                CloudState::Cloudy
            },
            reason: if cloud <= f64::from(threshold) {
                "cloud_cover_clear"
            } else {
                "cloud_cover_above_threshold"
            },
            valid_for_ms: remaining.min(CLOUD_DECISION_MS),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct WeatherForecast {
    pub provider: ForecastProvider,
    pub attribution: &'static str,
    pub fetched_at_ms: u64,
    pub expires_at_ms: u64,
    pub hour: Option<ForecastHour>,
    pub fresh: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct RigWeather {
    pub rig_id: Uuid,
    pub site_id: Option<Uuid>,
    pub evaluated_at_ms: u64,
    pub cloud_threshold_percent: u8,
    pub dew_limit_celsius: Option<u8>,
    pub reported: Option<WeatherObservation>,
    pub forecast: Option<WeatherForecast>,
    pub reported_cloud: CloudAssessment,
    pub forecast_conditions: CloudAssessment,
}

pub(super) fn create_tables(conn: &Connection) -> Result<(), Error> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS rig_weather(
        rig_id TEXT NOT NULL REFERENCES rig(id) ON DELETE CASCADE, profile_id TEXT NOT NULL,
        payload TEXT NOT NULL, PRIMARY KEY(rig_id,profile_id));
        CREATE TABLE IF NOT EXISTS rig_weather_history(
        rig_id TEXT NOT NULL REFERENCES rig(id) ON DELETE CASCADE,
        profile_id TEXT NOT NULL, minute_ms INTEGER NOT NULL, payload TEXT NOT NULL,
        PRIMARY KEY(rig_id,profile_id,minute_ms));
        CREATE INDEX IF NOT EXISTS rig_weather_history_time ON rig_weather_history(rig_id,minute_ms DESC);",
    )?;
    validate_tables(conn)
}

pub(super) fn validate_tables(conn: &Connection) -> Result<(), Error> {
    for sql in [
        "SELECT rig_id,profile_id,payload FROM rig_weather LIMIT 0",
        "SELECT rig_id,profile_id,minute_ms,payload FROM rig_weather_history LIMIT 0",
    ] {
        conn.prepare(sql).map_err(|_| Error::CorruptDatabase)?;
    }
    Ok(())
}

impl MetaStore {
    pub fn weather_observation(&self, rig: Uuid) -> Result<Option<WeatherObservation>, Error> {
        self.weather_observation_for(rig, None)
    }

    fn weather_observation_for(
        &self,
        rig: Uuid,
        profile: Option<Uuid>,
    ) -> Result<Option<WeatherObservation>, Error> {
        valid_id(rig)?;
        let value: Option<String> = self.connection.query_row(
            "SELECT substr(payload,1,8193) FROM rig_weather WHERE rig_id=?1 AND (?2 IS NULL OR profile_id=?2)
             ORDER BY json_extract(payload,'$.received_at_ms') DESC LIMIT 1", params![rig.to_string(), profile.map(|id| id.to_string())], |row| row.get(0)
        ).optional()?;
        value
            .map(|text| {
                if text.len() > 8192 {
                    return Err(Error::CorruptDatabase);
                }
                let observation: WeatherObservation =
                    serde_json::from_str(&text).map_err(|_| Error::CorruptDatabase)?;
                if observation.rig_id != rig
                    || profile.is_some_and(|id| observation.sample.profile_id != id)
                {
                    return Err(Error::CorruptDatabase);
                }
                observation
                    .sample
                    .validate(observation.received_at_ms)
                    .map_err(|_| Error::CorruptDatabase)?;
                Ok(observation)
            })
            .transpose()
    }

    /// Retain the latest observation and one sample per minute for 30 days.
    /// Retries and older reports cannot refresh a sample's freshness.
    pub fn record_weather(
        &mut self,
        rig: Uuid,
        sample: WeatherSample,
        now: u64,
    ) -> Result<bool, Error> {
        valid_id(rig)?;
        sample.validate(now)?;
        self.writing_liveness(|store| {
            if store.rig(rig)?.is_none() { return Err(Error::NotFound); }
            if let Some(previous) = store.weather_observation_for(rig, Some(sample.profile_id))? {
                if previous.sample.sample_id == sample.sample_id {
                    return if previous.sample == sample { Ok(false) } else { Err(Error::Conflict) };
                }
                if previous.received_at_ms <= now && previous.sample.reported_at_ms <= now.saturating_add(5_000)
                    && sample.reported_at_ms <= previous.sample.reported_at_ms {
                    return Ok(false);
                }
            }
            let value = WeatherObservation { rig_id: rig, sample, received_at_ms: now };
            let payload = serde_json::to_string(&value).map_err(|_| Error::InvalidInput)?;
            let tx = store.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute("INSERT INTO rig_weather VALUES(?1,?2,?3) ON CONFLICT(rig_id,profile_id) DO UPDATE SET payload=excluded.payload",
                params![rig.to_string(), value.sample.profile_id.to_string(), payload])?;
            tx.execute("INSERT INTO rig_weather_history VALUES(?1,?2,?3,?4) ON CONFLICT(rig_id,profile_id,minute_ms) DO UPDATE SET payload=excluded.payload",
                params![rig.to_string(), value.sample.profile_id.to_string(), (now / 60_000 * 60_000) as i64, payload])?;
            tx.execute("DELETE FROM rig_weather_history WHERE rig_id=?1 AND minute_ms<?2",
                params![rig.to_string(), now.saturating_sub(RETENTION_MS) as i64])?;
            tx.commit()?;
            Ok(true)
        })
    }

    pub fn weather_history(
        &self,
        rig: Uuid,
        from_ms: u64,
        to_ms: u64,
    ) -> Result<Vec<WeatherObservation>, Error> {
        valid_id(rig)?;
        if self.rig(rig)?.is_none() {
            return Err(Error::NotFound);
        }
        if from_ms > to_ms || to_ms > i64::MAX as u64 {
            return Err(Error::InvalidInput);
        }
        let mut query = self.connection.prepare("SELECT substr(payload,1,8193) FROM rig_weather_history WHERE rig_id=?1 AND minute_ms>=?2 AND minute_ms<=?3 ORDER BY minute_ms DESC LIMIT 1440")?;
        query
            .query_map(
                params![
                    rig.to_string(),
                    (from_ms / 60_000 * 60_000) as i64,
                    to_ms as i64
                ],
                |row| row.get::<_, String>(0),
            )?
            .map(|row| {
                let text = row?;
                if text.len() > 8192 {
                    return Err(Error::CorruptDatabase);
                }
                let observation: WeatherObservation =
                    serde_json::from_str(&text).map_err(|_| Error::CorruptDatabase)?;
                if observation.rig_id != rig {
                    return Err(Error::CorruptDatabase);
                }
                observation
                    .sample
                    .validate(observation.received_at_ms)
                    .map_err(|_| Error::CorruptDatabase)?;
                Ok(observation)
            })
            .collect()
    }

    /// Read cached site forecasts only: this never calls a weather provider.
    pub fn rig_weather(
        &self,
        rig: Uuid,
        profile: Option<Uuid>,
        now: u64,
    ) -> Result<RigWeather, Error> {
        if self.rig(rig)?.is_none() {
            return Err(Error::NotFound);
        }
        let location = self.rig_site(rig, self.rig_profile(rig)?.as_ref())?;
        let site_id = location.site.map(|site| site.id);
        let site_profile = site_id
            .map(|id| self.site_profile(id))
            .transpose()?
            .flatten();
        let settings = site_profile.as_ref().and_then(|site| site.forecast);
        let forecast_location = site_profile
            .as_ref()
            .and_then(|site| site.location.as_ref())
            .map(|location| location.value);
        let location_problem = match (location.location, forecast_location) {
            (Some(rig), Some(site))
                if forecast_coordinates(rig.latitude_degrees, rig.longitude_degrees)
                    == forecast_coordinates(site.latitude_degrees, site.longitude_degrees) =>
            {
                None
            }
            (Some(_), Some(_)) => Some("forecast_location_mismatch"),
            _ => Some("forecast_location_unavailable"),
        };
        let threshold = settings.map_or(DEFAULT_CLOUD_THRESHOLD_PERCENT, |s| {
            s.cloud_threshold_percent
        });
        let reported = self.weather_observation_for(rig, profile)?;
        let reported_cloud = reported.as_ref().map_or_else(
            || CloudAssessment::unknown("no_reported_weather"),
            |r| {
                if profile.is_some_and(|id| id != r.sample.profile_id) {
                    return CloudAssessment::unknown("weather_profile_changed");
                }
                if !r.sample.connected {
                    return CloudAssessment::unknown("weather_source_disconnected");
                }
                let until = r
                    .sample
                    .reported_at_ms
                    .saturating_add(WEATHER_MAX_AGE_MS)
                    .min(r.received_at_ms.saturating_add(WEATHER_MAX_AGE_MS));
                if now < r.received_at_ms || now < r.sample.reported_at_ms || now >= until {
                    return CloudAssessment::unknown("reported_weather_stale");
                }
                r.sample.values.cloud_percent.map_or_else(
                    || CloudAssessment::unknown("cloud_cover_unavailable"),
                    |cloud| CloudAssessment::assess(cloud, threshold, until - now),
                )
            },
        );
        let kept = site_id
            .map(|id| self.site_forecast(id))
            .transpose()?
            .flatten()
            .filter(|f| settings.is_some_and(|s| s.provider == f.provider));
        let forecast = kept.map(|f| WeatherForecast {
            provider: f.provider,
            attribution: f.provider.attribution(),
            fetched_at_ms: f.fetched_at_ms,
            expires_at_ms: f.expires_at_ms,
            fresh: now >= f.fetched_at_ms && f.fresh(now),
            hour: f
                .hours
                .into_iter()
                .find(|h| h.start_ms <= now && now < h.end_ms),
        });
        let forecast_conditions = forecast.as_ref().map_or_else(
            || CloudAssessment::unknown("no_site_forecast"),
            |f| {
                if let Some(reason) = location_problem {
                    return CloudAssessment::unknown(reason);
                }
                if !f.fresh {
                    return CloudAssessment::unknown("site_forecast_stale");
                }
                f.hour.map_or_else(
                    || CloudAssessment::unknown("forecast_does_not_cover_now"),
                    |h| {
                        let mut assessment = CloudAssessment::assess(
                            h.cloud_total,
                            threshold,
                            h.end_ms.min(f.expires_at_ms) - now,
                        );
                        if assessment.state == CloudState::Clear
                            && settings.is_some_and(|policy| !policy.clear(&h))
                        {
                            assessment.state = CloudState::Dewy;
                            assessment.reason = "dew_limit_reached";
                        }
                        assessment
                    },
                )
            },
        );
        Ok(RigWeather {
            rig_id: rig,
            site_id,
            evaluated_at_ms: now,
            cloud_threshold_percent: threshold,
            dew_limit_celsius: settings.and_then(|s| s.dew_limit_celsius),
            reported,
            forecast,
            reported_cloud,
            forecast_conditions,
        })
    }
}
