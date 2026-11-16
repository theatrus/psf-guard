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
mod hrz;
mod meridian;
pub use hrz::HrzError;
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

impl Site {
    pub fn validate(&self) -> Result<(), VisibilityError> {
        if !bounded(self.latitude_degrees, -90.0, 90.0)
            || !bounded(self.longitude_degrees, -180.0, 180.0)
            || !bounded(self.elevation_meters, -1000.0, 100_000.0)
        {
            return Err(VisibilityError::InvalidSite);
        }
        Ok(())
    }
}

impl AltitudeLimits {
    pub fn validate(&self) -> Result<(), VisibilityError> {
        if !bounded(self.rig_minimum_degrees, -90.0, 90.0)
            || !bounded(self.project_minimum_degrees, -90.0, 90.0)
            || !bounded(self.rig_maximum_degrees, -90.0, 90.0)
            || !bounded(self.project_maximum_degrees, -90.0, 90.0)
            || !bounded(self.horizon_offset_degrees, 0.0, 180.0)
            || self.rig_maximum_degrees <= self.rig_minimum_degrees
            || self.project_maximum_degrees <= self.project_minimum_degrees
        {
            return Err(VisibilityError::InvalidAltitudeLimits);
        }
        Ok(())
    }
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
        Ok(self.curve_at(azimuth_degrees))
    }

    /// [`Horizon::altitude`] of a horizon already validated, at a finite
    /// azimuth.
    fn curve_at(&self, azimuth_degrees: f64) -> Option<f64> {
        let Self::Custom { points } = self else {
            return None;
        };
        let azimuth = azimuth_degrees.rem_euclid(360.0);
        // Rust's remainder can round an extremely small negative input to 360.
        // NINA uses the same floating-point result rather than snapping to zero.
        let upper = points.partition_point(|p| p.azimuth_degrees < azimuth);
        if upper == 0 {
            return Some(points[0].altitude_degrees);
        }
        let right = points[upper];
        let left = points[upper - 1];
        let fraction =
            (azimuth - left.azimuth_degrees) / (right.azimuth_degrees - left.azimuth_degrees);
        Some(left.altitude_degrees + fraction * (right.altitude_degrees - left.altitude_degrees))
    }

    /// The lowest and highest altitude anywhere on a validated curve:
    /// `None` without one.
    fn curve_extent(&self) -> Option<(f64, f64)> {
        let Self::Custom { points } = self else {
            return None;
        };
        let altitudes = points.iter().map(|p| p.altitude_degrees);
        Some((
            altitudes.clone().fold(f64::INFINITY, f64::min),
            altitudes.fold(f64::NEG_INFINITY, f64::max),
        ))
    }

    /// The lowest and highest altitude of a validated curve between two
    /// azimuths, `0 <= from <= to < 360`: `None` without a curve. The
    /// curve is straight between its points, so its extremes are at the
    /// two ends and at the points between them.
    fn curve_range(&self, from: f64, to: f64) -> Option<(f64, f64)> {
        let Self::Custom { points } = self else {
            return None;
        };
        let (a, b) = (self.curve_at(from)?, self.curve_at(to)?);
        let (mut low, mut high) = (a.min(b), a.max(b));
        let first = points.partition_point(|p| p.azimuth_degrees <= from);
        let last = points.partition_point(|p| p.azimuth_degrees < to);
        for point in points.get(first..last).unwrap_or(&[]) {
            low = low.min(point.altitude_degrees);
            high = high.max(point.altitude_degrees);
        }
        Some((low, high))
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
    limits.validate()?;
    if !bounded(altitude_degrees, -90.0, 90.0) {
        return Err(VisibilityError::InvalidPosition);
    }
    let curve = horizon.altitude(azimuth_degrees)?;
    Ok(inside(limits, curve, altitude_degrees))
}

/// Whether an altitude is strictly inside valid limits, above the horizon
/// curve's altitude at its azimuth when there is a curve.
fn inside(limits: AltitudeLimits, curve: Option<f64>, altitude_degrees: f64) -> bool {
    let mut minimum = limits
        .rig_minimum_degrees
        .max(limits.project_minimum_degrees);
    if let Some(curve) = curve {
        minimum = minimum.max(curve + limits.horizon_offset_degrees);
    }
    altitude_degrees > minimum
        && altitude_degrees
            < limits
                .rig_maximum_degrees
                .min(limits.project_maximum_degrees)
}

/// [`altitude_allowed`] for many directions under one horizon and one set
/// of limits. [`altitude_allowed`] validates both on every call, which for
/// a horizon curve is a pass over all its points; this validates them once,
/// so each direction costs a lookup in the curve. The answers are the same.
#[derive(Clone, Copy, Debug)]
pub struct AltitudeTest<'a> {
    horizon: &'a Horizon,
    limits: AltitudeLimits,
}

impl<'a> AltitudeTest<'a> {
    /// Limits are checked before the horizon, as [`altitude_allowed`]
    /// checks them.
    pub fn new(horizon: &'a Horizon, limits: AltitudeLimits) -> Result<Self, VisibilityError> {
        limits.validate()?;
        horizon.validate()?;
        Ok(Self { horizon, limits })
    }

    pub fn allowed(
        &self,
        azimuth_degrees: f64,
        altitude_degrees: f64,
    ) -> Result<bool, VisibilityError> {
        if !bounded(altitude_degrees, -90.0, 90.0) || !azimuth_degrees.is_finite() {
            return Err(VisibilityError::InvalidPosition);
        }
        Ok(inside(
            self.limits,
            self.horizon.curve_at(azimuth_degrees),
            altitude_degrees,
        ))
    }

    /// The limits as two altitudes: the higher minimum and the lower
    /// maximum.
    fn floor_and_ceiling(&self) -> (f64, f64) {
        let limits = self.limits;
        (
            limits
                .rig_minimum_degrees
                .max(limits.project_minimum_degrees),
            limits
                .rig_maximum_degrees
                .min(limits.project_maximum_degrees),
        )
    }

    /// The span of the altitude a direction must clear, over every
    /// azimuth, and the altitude it must stay under: the lowest and the
    /// highest minimum, then the maximum.
    pub(crate) fn extent(&self) -> (f64, f64, f64) {
        let (floor, ceiling) = self.floor_and_ceiling();
        match self.horizon.curve_extent() {
            Some((low, high)) => {
                let offset = self.limits.horizon_offset_degrees;
                (floor.max(low + offset), floor.max(high + offset), ceiling)
            }
            None => (floor, floor, ceiling),
        }
    }

    /// For each whole degree of azimuth, the lowest and the highest
    /// altitude a direction must clear anywhere from a degree before it
    /// to a degree past its end: `None` without a curve. A direction
    /// known to within a degree of azimuth is above the minimum if it is
    /// above the highest, and under it if it is under the lowest. Across
    /// north both of the curve's ends count.
    pub(crate) fn minimum_by_degree(&self) -> Option<Vec<(f64, f64)>> {
        let Horizon::Custom { points } = self.horizon else {
            return None;
        };
        let (floor, _) = self.floor_and_ceiling();
        let offset = self.limits.horizon_offset_degrees;
        // The curve from an azimuth round to north, its last point included.
        let to_north = |from: f64| {
            let start = self.horizon.curve_at(from).unwrap_or(f64::NAN);
            points
                .iter()
                .filter(|p| p.azimuth_degrees > from)
                .fold((start, start), |(low, high), p| {
                    (low.min(p.altitude_degrees), high.max(p.altitude_degrees))
                })
        };
        let join = |a: (f64, f64), b: (f64, f64)| (a.0.min(b.0), a.1.max(b.1));
        (0..360)
            .map(|degree| {
                let (from, to) = (f64::from(degree) - 1.0, f64::from(degree) + 2.0);
                let (low, high) = if from < 0.0 {
                    join(to_north(from + 360.0), self.horizon.curve_range(0.0, to)?)
                } else if to >= 360.0 {
                    join(to_north(from), self.horizon.curve_range(0.0, to - 360.0)?)
                } else {
                    self.horizon.curve_range(from, to)?
                };
                Some((floor.max(low + offset), floor.max(high + offset)))
            })
            .collect::<Option<Vec<_>>>()
    }

    /// What [`AltitudeTest::allowed`] answers for every direction within
    /// `margin_degrees` on the sky of this one, when it is the same for
    /// all of them: `None` when some may be allowed and some not, or when
    /// it is not worth working out, as beside the zenith or across the
    /// curve's ends at north. A caller that knows a direction only to
    /// within the margin takes `Some` as certain and otherwise asks
    /// [`AltitudeTest::allowed`] with the direction itself.
    pub(crate) fn sure(
        &self,
        azimuth_degrees: f64,
        altitude_degrees: f64,
        margin_degrees: f64,
    ) -> Option<bool> {
        let (floor, ceiling) = self.floor_and_ceiling();
        if altitude_degrees > ceiling + margin_degrees {
            return Some(false);
        }
        // The margin on the sky is a wider span of azimuth higher up.
        let spread = altitude_degrees.to_radians().cos();
        if spread.is_nan() || spread < 0.02 {
            return None;
        }
        let reach = margin_degrees / spread;
        let (lowest, highest) = match self.horizon {
            Horizon::FixedMinimum {} => (floor, floor),
            Horizon::Custom { .. } => {
                let (from, to) = (azimuth_degrees - reach, azimuth_degrees + reach);
                // The curve's two ends may differ, so across north is left
                // to the caller.
                if !(from >= 0.0 && to < 360.0) {
                    return None;
                }
                let (low, high) = self.horizon.curve_range(from, to)?;
                let offset = self.limits.horizon_offset_degrees;
                (floor.max(low + offset), floor.max(high + offset))
            }
        };
        if altitude_degrees < lowest - margin_degrees {
            Some(false)
        } else if altitude_degrees > highest + margin_degrees
            && altitude_degrees < ceiling - margin_degrees
        {
            Some(true)
        } else {
            None
        }
    }
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

/// The star-independent part of an observation: everything about the site,
/// the moment and the Earth, prepared once so several bodies can be observed
/// at the same instant for the price of one. The result is the same as
/// [`observe`] to the last bit; SOFA's `atco13` is this followed by the
/// per-body steps.
#[derive(Clone, Copy)]
pub struct Observer {
    astrom: sofars::astro::IauAstrom,
}

impl Observer {
    pub fn at(
        site: Site,
        orientation: EarthOrientation,
        unix_ms: u64,
    ) -> Result<Self, VisibilityError> {
        site.validate()?;
        check_orientation(orientation, unix_ms)?;
        let (utc1, utc2) = utc_two_part(unix_ms)?;
        let mut astrom = sofars::astro::IauAstrom::default();
        let mut eo = 0.0;
        sofars::astro::apco13(
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
            &mut astrom,
            &mut eo,
        )
        .map_err(|_| VisibilityError::AstronomyUnavailable)?;
        Ok(Self { astrom })
    }

    /// How far the Earth has turned under the stars at this site and
    /// instant, in radians: the angle a hour angle is measured from. It
    /// grows by a turn each sidereal day, whatever the clock does.
    pub(crate) fn rotation(&self) -> f64 {
        self.astrom.eral
    }

    pub fn observe(&self, position: IcrsPosition) -> Result<ObservedPosition, VisibilityError> {
        self.observe_with_declination(position)
            .map(|(observed, _)| observed)
    }

    /// [`Observer::observe`], with the declination the site sees the
    /// position at, in degrees: with its hour angle, where it is among the
    /// stars as they turn overhead.
    pub(crate) fn observe_with_declination(
        &self,
        position: IcrsPosition,
    ) -> Result<(ObservedPosition, f64), VisibilityError> {
        if !bounded(position.ra_degrees, 0.0, 360.0)
            || position.ra_degrees == 360.0
            || !bounded(position.dec_degrees, -90.0, 90.0)
        {
            return Err(VisibilityError::InvalidPosition);
        }
        let (ri, di) = sofars::astro::atciq(
            position.ra_degrees.to_radians(),
            position.dec_degrees.to_radians(),
            0.0,
            0.0,
            0.0,
            0.0,
            // `atciq` only reads it, but asks for it mutably.
            &mut self.astrom.clone(),
        );
        let (azimuth, zenith, hour_angle, declination, _) =
            sofars::astro::atioq(ri, di, &self.astrom);
        finish(azimuth, zenith, hour_angle, declination)
    }
}

fn check_orientation(orientation: EarthOrientation, unix_ms: u64) -> Result<(), VisibilityError> {
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
    Ok(())
}

/// UTC as SOFA's two-part Julian date, through 2099.
fn utc_two_part(unix_ms: u64) -> Result<(f64, f64), VisibilityError> {
    let millis = i64::try_from(unix_ms).map_err(|_| VisibilityError::UnsupportedTime)?;
    let time =
        chrono::DateTime::from_timestamp_millis(millis).ok_or(VisibilityError::UnsupportedTime)?;
    // The leap-second table ends with the one at the start of 2017, and
    // SOFA keeps TAI-UTC at 37 s after it. A leap second the table misses
    // moves TT by one second: the Sun, the Moon and precession shift by
    // under an arcsecond, and Earth rotation not at all, since UT1 comes
    // from the caller's UT1-UTC. So later years assume no new leap seconds
    // rather than fail; the CGPM plans to stop adding them by 2035. The end
    // stays inside epv00's 1900-2100 range, whose dependency wrapper unwraps
    // errors.
    if !(1970..=2099).contains(&time.year()) {
        return Err(VisibilityError::UnsupportedTime);
    }
    sofars::ts::dtf2d(
        "UTC",
        time.year(),
        time.month() as i32,
        time.day() as i32,
        time.hour() as i32,
        time.minute() as i32,
        f64::from(time.second()) + f64::from(time.timestamp_subsec_millis()) / 1000.0,
    )
    .map_err(|_| VisibilityError::AstronomyUnavailable)
}

fn finish(
    azimuth: f64,
    zenith: f64,
    hour_angle: f64,
    declination: f64,
) -> Result<(ObservedPosition, f64), VisibilityError> {
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
    site.validate()?;
    check_orientation(orientation, unix_ms)?;
    let (utc1, utc2) = utc_two_part(unix_ms)?;
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
    finish(azimuth, zenith, hour_angle, declination)
}

#[cfg(test)]
mod observer_tests {
    use super::*;

    #[test]
    fn a_prepared_observer_matches_the_one_shot_observation_to_the_bit() {
        let site = Site {
            latitude_degrees: 34.2,
            longitude_degrees: -118.3,
            elevation_meters: 400.0,
        };
        let orientation = EarthOrientation {
            ut1_minus_utc_seconds: 0.0,
            polar_motion_x_radians: 0.0,
            polar_motion_y_radians: 0.0,
            valid_from_ms: 1_700_000_000_000,
            valid_until_ms: 1_800_000_000_000,
        };
        let bodies = [
            IcrsPosition {
                ra_degrees: 38.2,
                dec_degrees: 61.45,
            },
            IcrsPosition {
                ra_degrees: 314.75,
                dec_degrees: 44.37,
            },
            IcrsPosition {
                ra_degrees: 0.0,
                dec_degrees: -89.0,
            },
            IcrsPosition {
                ra_degrees: 359.9,
                dec_degrees: 5.0,
            },
        ];
        for t in [1_759_000_000_000_u64, 1_759_000_300_000, 1_762_345_678_901] {
            let prepared = Observer::at(site, orientation, t).unwrap();
            for body in bodies {
                let one_shot = observe(body, site, orientation, t).unwrap();
                let shared = prepared.observe(body).unwrap();
                assert_eq!(one_shot, shared, "{body:?} at {t}");
            }
        }
        assert!(matches!(
            Observer::at(site, orientation, 1_000).map(|_| ()),
            Err(VisibilityError::StaleEarthOrientation)
        ));
    }
}

#[cfg(test)]
mod sure_tests {
    use super::*;

    #[test]
    fn a_sure_answer_holds_for_every_direction_within_the_margin() {
        let point = |azimuth_degrees: f64, altitude_degrees: f64| HorizonPoint {
            azimuth_degrees,
            altitude_degrees,
        };
        // A wall, a spike half a degree wide, and ends that differ at north.
        let curve = Horizon::Custom {
            points: vec![
                point(0.0, 30.0),
                point(40.0, 5.0),
                point(99.9, 5.0),
                point(100.0, 60.0),
                point(130.0, 60.0),
                point(130.1, 5.0),
                point(219.75, 5.0),
                point(220.0, 75.0),
                point(220.25, 5.0),
                point(360.0, 12.0),
            ],
        };
        let flat = Horizon::FixedMinimum {};
        let limits = AltitudeLimits {
            rig_minimum_degrees: 10.0,
            project_minimum_degrees: -90.0,
            horizon_offset_degrees: 2.0,
            rig_maximum_degrees: 80.0,
            project_maximum_degrees: 90.0,
        };
        let margin = 0.01;
        let (mut sure, mut unsure) = (0, 0);
        for horizon in [&curve, &flat] {
            let test = AltitudeTest::new(horizon, limits).unwrap();
            let (lowest, highest, maximum) = test.extent();
            assert_eq!(maximum, 80.0);
            if matches!(horizon, Horizon::Custom { .. }) {
                assert_eq!((lowest, highest), (10.0, 77.0));
            } else {
                assert_eq!((lowest, highest), (10.0, 10.0));
            }
            // Azimuths every 0.07° and altitudes every 0.013°, so some land
            // inside the margin of every edge.
            for a in 0..5143 {
                let azimuth = f64::from(a) * 0.07;
                for h in 0..230 {
                    let altitude = -5.0 + f64::from(h) * 0.413 + f64::from(a % 31) * 0.013;
                    if altitude >= 90.0 {
                        continue;
                    }
                    let Some(answer) = test.sure(azimuth, altitude, margin) else {
                        unsure += 1;
                        continue;
                    };
                    sure += 1;
                    let reach = margin / altitude.to_radians().cos();
                    for (across, up) in [
                        (0.0, 0.0),
                        (-1.0, -1.0),
                        (-1.0, 1.0),
                        (1.0, -1.0),
                        (1.0, 1.0),
                        (0.5, -1.0),
                        (-0.3, 1.0),
                        (1.0, 0.0),
                        (-1.0, 0.2),
                    ] {
                        let near = (
                            azimuth + across * reach * 0.999,
                            (altitude + up * margin * 0.999).clamp(-90.0, 90.0),
                        );
                        assert_eq!(
                            test.allowed(near.0, near.1).unwrap(),
                            answer,
                            "{azimuth},{altitude} sure of {answer}, not so at {near:?}"
                        );
                    }
                }
            }
        }
        // The spans by whole degree hold every minimum within a degree.
        let test = AltitudeTest::new(&curve, limits).unwrap();
        let spans = test.minimum_by_degree().unwrap();
        assert_eq!(spans.len(), 360);
        assert!(AltitudeTest::new(&flat, limits)
            .unwrap()
            .minimum_by_degree()
            .is_none());
        for a in 0..36_000 {
            let azimuth = f64::from(a) * 0.01;
            let (lowest, highest) = spans[azimuth as usize];
            for near in [-0.999, -0.5, 0.0, 0.5, 0.999] {
                let curve_there = curve.altitude(azimuth + near).unwrap().unwrap();
                let minimum = 10.0_f64.max(curve_there + 2.0);
                assert!(
                    (lowest..=highest).contains(&minimum),
                    "{azimuth}{near:+}: {minimum} outside {lowest}..{highest}"
                );
            }
        }
        // Both ends at north count either side of it, and a flat stretch
        // is not widened by the wall three degrees off.
        for north in [spans[0], spans[359]] {
            assert_eq!(north.1, 32.0);
            assert!((13.8..14.0).contains(&north.0), "{north:?}");
        }
        assert_eq!(spans[60], (10.0, 10.0));
        assert_eq!(spans[100], (10.0, 62.0));
        // Most directions are sure, and those on an edge are not.
        assert!(sure > 20 * unsure && unsure > 1000, "{sure} {unsure}");
        // Beside the zenith and across north nothing is claimed.
        let open = AltitudeLimits {
            rig_maximum_degrees: 90.0,
            ..limits
        };
        let to_the_zenith = AltitudeTest::new(&curve, open).unwrap();
        assert_eq!(to_the_zenith.sure(50.0, 89.5, margin), None);
        let test = AltitudeTest::new(&curve, limits).unwrap();
        assert_eq!(test.sure(0.001, 50.0, margin), None);
        assert_eq!(test.sure(359.999, 50.0, margin), None);
        // Over the maximum is out at any azimuth.
        assert_eq!(test.sure(0.001, 85.0, margin), Some(false));
    }
}
