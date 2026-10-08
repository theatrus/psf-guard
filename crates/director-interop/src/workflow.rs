//! Host-neutral commissioning and observing-night requests. No credentials or IO.
use crate::{
    astrocollab::{self, Source},
    json, Error,
};
use chrono::NaiveDate;
use psf_guard_director_core::optics::{Optics, Rotation};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FilterSetup {
    pub exposure_seconds: f64,
    pub bandpass_nm: Option<f64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub binning: u32,
    pub colour: bool,
    pub hours_per_night: f64,
    pub filters: BTreeMap<String, FilterSetup>,
    /// Only current activity; no target names or coordinates are disclosed.
    pub share_status: bool,
}
impl Settings {
    pub fn validate(&self) -> Result<(), Error> {
        if !(1..=16).contains(&self.binning)
            || !finite(self.hours_per_night, 0.01, 24.0)
            || self.filters.is_empty()
            || self.filters.len() > 32
        {
            return Err(Error::InvalidValue);
        }
        for (name, filter) in &self.filters {
            if name.is_empty()
                || name.len() > 80
                || name.trim() != name
                || name.chars().any(char::is_control)
                || !finite(filter.exposure_seconds, 0.001, 86400.0)
                || filter.bandpass_nm.is_some_and(|v| !finite(v, 0.01, 1e6))
            {
                return Err(Error::InvalidValue);
            }
        }
        // The public protocol's aliases, not the local planner's filter aliases.
        astrocollab::decode_hello_profile(&serde_json::to_vec(&json!({"profile":{
            "filters":self.filters.iter().map(|(k,v)|(k.clone(),json!(v.bandpass_nm))).collect::<BTreeMap<_,_>>()
        }})).map_err(|_| Error::InvalidValue)?)?;
        Ok(())
    }
    pub fn hello(
        &self,
        optics: &Optics,
        name: &str,
        presence: Option<Value>,
    ) -> Result<Value, Error> {
        self.validate()?;
        optics.validate().map_err(|_| Error::InvalidValue)?;
        let rotation = match optics.rotation {
            Rotation::Fixed { angle_degrees } | Rotation::Manual { angle_degrees } => {
                json!(angle_degrees)
            }
            Rotation::Rotator {} => Value::Null,
        };
        let body = json!({"protocol":1,"profile":{
            "name":name,"focalLength":optics.focal_length_mm,"pixelSize":optics.pixel_size_um,
            "sensorWidth":optics.sensor_width_px,"sensorHeight":optics.sensor_height_px,
            "binning":self.binning,"colour":self.colour,"rotation":rotation,
            "hoursPerNight":self.hours_per_night,
            "filters":self.filters.iter().map(|(k,v)|(k.clone(),json!(v.bandpass_nm))).collect::<BTreeMap<_,_>>(),
            "exposures":self.exposures(),
        },"presence":presence});
        astrocollab::decode_hello_profile(
            &serde_json::to_vec(&body).map_err(|_| Error::InvalidValue)?,
        )?;
        Ok(body)
    }
    pub fn exposures(&self) -> BTreeMap<String, f64> {
        self.filters
            .iter()
            .map(|(k, v)| (k.clone(), v.exposure_seconds))
            .collect()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Night {
    pub night: String,
    pub moon: f64,
    pub moon_up: f64,
}
impl Night {
    /// A manual date override still derives lunar context from the rig's site.
    pub fn for_date(
        site: psf_guard_director_core::visibility::Site,
        date: &str,
    ) -> Result<Self, Error> {
        site.validate().map_err(|_| Error::InvalidNight)?;
        let parsed =
            NaiveDate::parse_from_str(date, "%Y-%m-%d").map_err(|_| Error::InvalidNight)?;
        if parsed.format("%Y-%m-%d").to_string() != date {
            return Err(Error::InvalidNight);
        }
        let local_evening = parsed
            .and_hms_opt(18, 0, 0)
            .ok_or(Error::InvalidNight)?
            .and_utc()
            .timestamp_millis();
        let offset = (site.longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
        Self::for_site(
            site,
            u64::try_from(
                local_evening
                    .checked_sub(offset)
                    .ok_or(Error::InvalidNight)?,
            )
            .map_err(|_| Error::InvalidNight)?,
        )
    }
    /// The rig's noon-to-noon observing night, not the coordinator's timezone.
    pub fn for_site(
        site: psf_guard_director_core::visibility::Site,
        now_ms: u64,
    ) -> Result<Self, Error> {
        use psf_guard_director_core::{
            night::{night_preview, NightRequest},
            visibility::{AltitudeLimits, Horizon},
            windows::MeridianExclusion,
        };
        let summary = night_preview(&NightRequest {
            site,
            horizon: Horizon::FixedMinimum {},
            limits: AltitudeLimits {
                rig_minimum_degrees: 0.0,
                project_minimum_degrees: 0.0,
                horizon_offset_degrees: 0.0,
                rig_maximum_degrees: 90.0,
                project_maximum_degrees: 90.0,
            },
            targets: vec![],
            start_ms: now_ms,
            nights: 1,
            step_ms: 300_000,
            dark_below_degrees: -18.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
            meridian_window_ms: 0,
        })
        .map_err(|_| Error::InvalidNight)?
        .pop()
        .ok_or(Error::InvalidNight)?;
        if summary.dark_hours <= 0.0 {
            return Err(Error::InvalidNight);
        }
        let night = Self {
            night: summary.date,
            moon: summary.moon_illumination,
            moon_up: (summary.moon_hours_up_in_dark / summary.dark_hours).clamp(0.0, 1.0),
        };
        night.validate()?;
        Ok(night)
    }
    pub fn validate(&self) -> Result<(), Error> {
        let date =
            NaiveDate::parse_from_str(&self.night, "%Y-%m-%d").map_err(|_| Error::InvalidNight)?;
        if date.format("%Y-%m-%d").to_string() != self.night
            || !finite(self.moon, 0.0, 1.0)
            || !finite(self.moon_up, 0.0, 1.0)
        {
            return Err(Error::InvalidNight);
        }
        Ok(())
    }
    pub fn query(&self) -> Result<Vec<(&'static str, String)>, Error> {
        self.validate()?;
        Ok(vec![
            ("night", self.night.clone()),
            ("moon", self.moon.to_string()),
            ("moonUp", self.moon_up.to_string()),
        ])
    }
    pub fn join(&self, settings: &Settings) -> Result<Value, Error> {
        self.validate()?;
        settings.validate()?;
        Ok(
            json!({"hours":0.0,"exposures":settings.exposures(),"night":self.night,"moon":self.moon,"moonUp":self.moon_up}),
        )
    }
}
pub fn hello_reply(bytes: &[u8], source: &Source) -> Result<(), Error> {
    let body = json::decode(bytes)?;
    if body["protocol"].as_u64() != Some(1)
        || body["agent"].as_str() != Some(source.agent_id())
        || body["serverTime"]
            .as_f64()
            .is_none_or(|v| !finite(v, 0.0, 4_102_444_800.0))
    {
        return Err(Error::InvalidReply);
    }
    Ok(())
}
pub fn valid_remote_id(id: &str) -> bool {
    astrocollab::validate_id(id).is_ok()
}
fn finite(v: f64, min: f64, max: f64) -> bool {
    v.is_finite() && (min..=max).contains(&v)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn automatic_night_uses_the_rigs_local_noon_and_computes_lunar_context() {
        use psf_guard_director_core::visibility::Site;
        let west = Site {
            latitude_degrees: 35.0,
            longitude_degrees: -105.0,
            elevation_meters: 2000.0,
        };
        let utc = chrono::DateTime::parse_from_rfc3339("2026-10-08T03:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        let automatic = Night::for_site(west, utc).unwrap();
        assert_eq!(automatic.night, "2026-10-07");
        assert!((0.0..=1.0).contains(&automatic.moon));
        assert!((0.0..=1.0).contains(&automatic.moon_up));
        let manual = Night::for_date(west, "2026-10-07").unwrap();
        assert_eq!(automatic.query().unwrap(), manual.query().unwrap());
        let east = Site {
            longitude_degrees: 150.0,
            latitude_degrees: -30.0,
            ..west
        };
        let utc = chrono::DateTime::parse_from_rfc3339("2026-10-07T11:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(Night::for_site(east, utc).unwrap().night, "2026-10-07");
        let morning = chrono::DateTime::parse_from_rfc3339("2026-10-08T15:00:00Z")
            .unwrap()
            .timestamp_millis() as u64;
        assert_eq!(Night::for_site(west, morning).unwrap().night, "2026-10-07");
        assert!(Night::for_date(west, "2026-1-2").is_err());
        assert!(Night::for_date(west, "2026-02-30").is_err());
        assert!(Night::for_date(
            Site {
                latitude_degrees: 89.0,
                ..west
            },
            "2026-06-21"
        )
        .is_err());
    }
    #[test]
    fn refuses_ambiguous_filters_paths_and_unnamed_nights() {
        let mut settings = Settings {
            binning: 1,
            colour: false,
            hours_per_night: 6.0,
            share_status: false,
            filters: BTreeMap::from([(
                "Ha".into(),
                FilterSetup {
                    exposure_seconds: 300.0,
                    bandpass_nm: Some(7.0),
                },
            )]),
        };
        assert!(settings.validate().is_ok());
        settings.filters.insert(
            "H".into(),
            FilterSetup {
                exposure_seconds: 300.0,
                bandpass_nm: Some(7.0),
            },
        );
        assert!(settings.validate().is_err());
        assert!(!valid_remote_id("../health"));
        assert!(Night {
            night: "2026-1-2".into(),
            moon: 0.0,
            moon_up: 0.0
        }
        .validate()
        .is_err());
    }
    #[test]
    fn hello_is_bound_to_original_agent() {
        let source = Source::new("https://example.com/", "000000000001", false).unwrap();
        assert!(hello_reply(
            br#"{"agent":"000000000001","protocol":1,"serverTime":1791171023.0}"#,
            &source
        )
        .is_ok());
        assert!(hello_reply(
            br#"{"agent":"000000000002","protocol":1,"serverTime":1791171023.0}"#,
            &source
        )
        .is_err());
    }
}
