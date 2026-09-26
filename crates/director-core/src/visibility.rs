//! Shared native-horizon evaluation and ICRS-to-observed coordinates.
//!
//! Astronomy uses the unmodified published sofars Rust translation of SOFA.
//! This wrapper validates inputs and converts units; it is not endorsed by SOFA.
//! No clock, network, file access, or device state is consulted. Point and span
//! calculations are not exposure/window authorization: observing-window construction,
//! darkness/conditions, transit searches and dispatch fencing remain separate.

use chrono::{Datelike, Timelike};
use serde::{Deserialize, Serialize};
mod span;
pub use span::{check_altitude_span, AltitudeSpan};
mod windows;
pub use windows::{altitude_windows, AltitudeWindows};
mod meridian;
pub use meridian::{meridian_windows, MeridianWindows};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HorizonPoint {
    pub azimuth_degrees: f64,
    pub altitude_degrees: f64,
}

/// Canonical NINA export: degrees, north=0/east=90, ordered endpoints 0 and
/// 360, linear segments and modulo-360 queries. Keep unequal endpoint values.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Horizon {
    FixedMinimum {},
    Custom { points: Vec<HorizonPoint> },
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AltitudeLimits {
    pub rig_minimum_degrees: f64,
    pub project_minimum_degrees: f64,
    /// A project can raise the required clearance, never lower the local curve.
    pub horizon_offset_degrees: f64,
    pub rig_maximum_degrees: f64,
    pub project_maximum_degrees: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Site {
    pub latitude_degrees: f64,
    /// East-positive longitude.
    pub longitude_degrees: f64,
    pub elevation_meters: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IcrsPosition {
    pub ra_degrees: f64,
    pub dec_degrees: f64,
}

/// Caller-supplied Earth orientation with explicit validity. Missing or stale
/// evidence is not zero correction. Site/ephemeris revision belongs in the
/// caller's configuration identity and eventual visibility cache key.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EarthOrientation {
    pub ut1_minus_utc_seconds: f64,
    pub polar_motion_x_radians: f64,
    pub polar_motion_y_radians: f64,
    pub valid_from_ms: u64,
    pub valid_until_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ObservedPosition {
    pub azimuth_degrees: f64,
    pub altitude_degrees: f64,
    pub hour_angle_degrees: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VisibilityError {
    InvalidHorizon,
    InvalidAltitudeLimits,
    InvalidPosition,
    InvalidSite,
    InvalidEarthOrientation,
    StaleEarthOrientation,
    EarthOrientationDiscontinuity,
    UnsupportedTime,
    AstronomyUnavailable,
    InvalidSpan,
    UnresolvedSpan,
    SpanBudgetExceeded,
    TooManyVisibilityWindows,
    MeridianTimeOverflow,
}

fn bounded(value: f64, minimum: f64, maximum: f64) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

impl Horizon {
    pub fn validate(&self) -> Result<(), VisibilityError> {
        let Self::Custom { points } = self else {
            return Ok(());
        };
        if !(2..=4098).contains(&points.len())
            || points[0].azimuth_degrees != 0.0
            || points[points.len() - 1].azimuth_degrees != 360.0
            || points.iter().any(|p| {
                !bounded(p.azimuth_degrees, 0.0, 360.0) || !bounded(p.altitude_degrees, -90.0, 90.0)
            })
            || points
                .windows(2)
                .any(|p| p[0].azimuth_degrees >= p[1].azimuth_degrees)
        {
            return Err(VisibilityError::InvalidHorizon);
        }
        Ok(())
    }

    /// None explicitly means no file curve, not a zero-degree replacement.
    pub fn altitude(&self, azimuth_degrees: f64) -> Result<Option<f64>, VisibilityError> {
        self.validate()?;
        if !azimuth_degrees.is_finite() {
            return Err(VisibilityError::InvalidPosition);
        }
        let Self::Custom { points } = self else {
            return Ok(None);
        };
        let azimuth = azimuth_degrees.rem_euclid(360.0);
        // Rust's remainder can round an extremely small negative input to 360.
        // NINA uses the same floating-point result rather than snapping to zero.
        let upper = points.partition_point(|p| p.azimuth_degrees < azimuth);
        if upper == 0 {
            return Ok(Some(points[0].altitude_degrees));
        }
        let right = points[upper];
        let left = points[upper - 1];
        let fraction =
            (azimuth - left.azimuth_degrees) / (right.azimuth_degrees - left.azimuth_degrees);
        Ok(Some(
            left.altitude_degrees + fraction * (right.altitude_degrees - left.altitude_degrees),
        ))
    }
}

/// Strict boundaries: equality at the horizon/minimum or maximum is not safe.
/// This evaluates one direction only; it cannot prove a whole exposure fits.
pub fn altitude_allowed(
    horizon: &Horizon,
    limits: AltitudeLimits,
    azimuth_degrees: f64,
    altitude_degrees: f64,
) -> Result<bool, VisibilityError> {
    if !bounded(limits.rig_minimum_degrees, -90.0, 90.0)
        || !bounded(limits.project_minimum_degrees, -90.0, 90.0)
        || !bounded(limits.rig_maximum_degrees, -90.0, 90.0)
        || !bounded(limits.project_maximum_degrees, -90.0, 90.0)
        || !bounded(limits.horizon_offset_degrees, 0.0, 180.0)
        || limits.rig_maximum_degrees <= limits.rig_minimum_degrees
        || limits.project_maximum_degrees <= limits.project_minimum_degrees
    {
        return Err(VisibilityError::InvalidAltitudeLimits);
    }
    if !bounded(altitude_degrees, -90.0, 90.0) {
        return Err(VisibilityError::InvalidPosition);
    }
    let curve = horizon.altitude(azimuth_degrees)?;
    let mut minimum = limits
        .rig_minimum_degrees
        .max(limits.project_minimum_degrees);
    if let Some(curve) = curve {
        minimum = minimum.max(curve + limits.horizon_offset_degrees);
    }
    Ok(altitude_degrees > minimum
        && altitude_degrees
            < limits
                .rig_maximum_degrees
                .min(limits.project_maximum_degrees))
}

/// SOFA ICRS J2000 to topocentric position without atmospheric refraction or
/// stellar proper motion/parallax. Matches NINA's zero-pressure transform mode.
/// UTC uses a two-part quasi-Julian date, including leap-day length. The pinned
/// dependency drops SOFA's dubious-year warning, so this wrapper bounds dates.
pub fn observe(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    unix_ms: u64,
) -> Result<ObservedPosition, VisibilityError> {
    observe_with_declination(position, site, orientation, unix_ms).map(|(observed, _)| observed)
}

fn observe_with_declination(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    unix_ms: u64,
) -> Result<(ObservedPosition, f64), VisibilityError> {
    if !bounded(position.ra_degrees, 0.0, 360.0)
        || position.ra_degrees == 360.0
        || !bounded(position.dec_degrees, -90.0, 90.0)
    {
        return Err(VisibilityError::InvalidPosition);
    }
    if !bounded(site.latitude_degrees, -90.0, 90.0)
        || !bounded(site.longitude_degrees, -180.0, 180.0)
        || !bounded(site.elevation_meters, -1000.0, 100_000.0)
    {
        return Err(VisibilityError::InvalidSite);
    }
    if !bounded(orientation.ut1_minus_utc_seconds, -1.0, 1.0)
        || !bounded(orientation.polar_motion_x_radians, -0.001, 0.001)
        || !bounded(orientation.polar_motion_y_radians, -0.001, 0.001)
        || orientation.valid_from_ms >= orientation.valid_until_ms
    {
        return Err(VisibilityError::InvalidEarthOrientation);
    }
    if unix_ms < orientation.valid_from_ms || unix_ms >= orientation.valid_until_ms {
        return Err(VisibilityError::StaleEarthOrientation);
    }
    let millis = i64::try_from(unix_ms).map_err(|_| VisibilityError::UnsupportedTime)?;
    let time =
        chrono::DateTime::from_timestamp_millis(millis).ok_or(VisibilityError::UnsupportedTime)?;
    // Match the SOFA 2023 model's five-year future horizon. This also stays
    // inside epv00's 1900-2100 range, whose dependency wrapper unwraps errors.
    // Updating the leap-second model requires an explicit reviewed change.
    if !(1970..=2028).contains(&time.year()) {
        return Err(VisibilityError::UnsupportedTime);
    }
    let (utc1, utc2) = sofars::ts::dtf2d(
        "UTC",
        time.year(),
        time.month() as i32,
        time.day() as i32,
        time.hour() as i32,
        time.minute() as i32,
        f64::from(time.second()) + f64::from(time.timestamp_subsec_millis()) / 1000.0,
    )
    .map_err(|_| VisibilityError::AstronomyUnavailable)?;
    let (azimuth, zenith, hour_angle, declination, _, _) = sofars::astro::atco13(
        position.ra_degrees.to_radians(),
        position.dec_degrees.to_radians(),
        0.0,
        0.0,
        0.0,
        0.0,
        utc1,
        utc2,
        orientation.ut1_minus_utc_seconds,
        site.longitude_degrees.to_radians(),
        site.latitude_degrees.to_radians(),
        site.elevation_meters,
        orientation.polar_motion_x_radians,
        orientation.polar_motion_y_radians,
        0.0,
        0.0,
        0.0,
        0.0,
    )
    .map_err(|_| VisibilityError::AstronomyUnavailable)?;
    let observed = ObservedPosition {
        azimuth_degrees: azimuth.to_degrees(),
        altitude_degrees: 90.0 - zenith.to_degrees(),
        hour_angle_degrees: hour_angle.to_degrees(),
    };
    if !bounded(observed.azimuth_degrees, 0.0, 360.0)
        || !bounded(observed.altitude_degrees, -90.0, 90.0)
        || !bounded(observed.hour_angle_degrees, -180.0, 180.0)
        || !bounded(declination.to_degrees(), -90.0, 90.0)
    {
        return Err(VisibilityError::AstronomyUnavailable);
    }
    Ok((observed, declination.to_degrees()))
}
