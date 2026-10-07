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
