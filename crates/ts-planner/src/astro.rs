// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The astronomy Target Scheduler plans with, reproduced with its quirks:
//! a target's altitude from its stored J2000 coordinates against apparent
//! sidereal time, with no precession and no refraction; transit times that
//! count sidereal hours as solar hours; and N.I.N.A.'s three-point fit for
//! the Sun's crossings. The Sun and Moon come from SOFA's ephemerides, of
//! date, as N.I.N.A.'s NOVAS calls give them.

use psf_guard_director_core::visibility::Site;

pub(crate) const HOUR_MS: i64 = 3_600_000;
pub(crate) const DAY_MS: i64 = 86_400_000;
const UNIX_EPOCH_JD: f64 = 2_440_587.5;
/// TT minus UTC since 2017, seconds.
const TT_MINUS_UTC_SECONDS: f64 = 69.184;
const AU_METERS: f64 = 149_597_870_700.0;
const LUNAR_CYCLE_DAYS: f64 = 29.53059;

fn ut(utc_ms: i64) -> (f64, f64) {
    (UNIX_EPOCH_JD, utc_ms as f64 / DAY_MS as f64)
}

fn tt(utc_ms: i64) -> (f64, f64) {
    (
        UNIX_EPOCH_JD,
        utc_ms as f64 / DAY_MS as f64 + TT_MINUS_UTC_SECONDS / 86_400.0,
    )
}

/// Equatorial coordinates of date, in hours and degrees.
#[derive(Clone, Copy, Debug)]
pub(crate) struct OfDate {
    pub(crate) ra_hours: f64,
    pub(crate) dec_degrees: f64,
}

/// One night's frame: the precession-nutation matrix and the equation of
/// the equinoxes, which move too little in a day to recompute.
#[derive(Clone, Debug)]
pub(crate) struct Frame {
    rnpb: [[f64; 3]; 3],
    equation_of_equinoxes: f64,
    site: Site,
    /// The observer's geocentric position on the Earth, metres.
    observer: [f64; 3],
    /// An observer at latitude 0, longitude 0, sea level, where N.I.N.A.
    /// measures the Moon's age.
    origin: [f64; 3],
}

impl Frame {
    pub(crate) fn new(site: Site, utc_ms: i64) -> Self {
        let (t1, t2) = tt(utc_ms);
        let rnpb = sofars::pnp::pnm06a(t1, t2);
        let equation_of_equinoxes = sofars::erst::ee06a(t1, t2);
        let observer = sofars::coords::gd2gc(
            1,
            site.longitude_degrees.to_radians(),
            site.latitude_degrees.to_radians(),
            site.elevation_meters,
        )
        .unwrap_or([0.0; 3]);
        let origin = sofars::coords::gd2gc(1, 0.0, 0.0, 0.0).unwrap_or([0.0; 3]);
        Self {
            rnpb,
            equation_of_equinoxes,
            site,
            observer,
            origin,
        }
    }

    pub(crate) fn latitude_degrees(&self) -> f64 {
        self.site.latitude_degrees
    }

    /// Greenwich apparent sidereal time, hours in [0, 24).
    pub(crate) fn gast_hours(&self, utc_ms: i64) -> f64 {
        let (u1, u2) = ut(utc_ms);
        let (t1, t2) = tt(utc_ms);
        let gast = sofars::erst::gmst06(u1, u2, t1, t2) + self.equation_of_equinoxes;
        (gast.to_degrees() / 15.0).rem_euclid(24.0)
    }

    /// N.I.N.A.'s local sidereal time: GAST plus the longitude in hours,
    /// **not** reduced into a day.
    pub(crate) fn lst_hours(&self, utc_ms: i64) -> f64 {
        self.gast_hours(utc_ms) + self.site.longitude_degrees / 15.0
    }

    /// Altitude and azimuth, degrees, of coordinates at an instant, by
    /// N.I.N.A.'s formulas: geometric, with no refraction.
    pub(crate) fn horizontal(&self, ra_hours: f64, dec_degrees: f64, utc_ms: i64) -> (f64, f64) {
        horizontal(
            self.lst_hours(utc_ms),
            ra_hours,
            dec_degrees,
            self.site.latitude_degrees,
        )
    }

    /// The transit Target Scheduler computes for the local midnight starting
    /// a date: that midnight plus 24 hours less the hour angle then, hours
    /// of sidereal time taken as hours of clock time.
    pub(crate) fn transit_after(&self, midnight_utc_ms: i64, ra_hours: f64) -> i64 {
        let hour_angle = hour_angle(self.lst_hours(midnight_utc_ms), ra_hours);
        midnight_utc_ms + ((24.0 - hour_angle) * HOUR_MS as f64).round() as i64
    }

    fn of_date(&self, gcrs: &[f64; 3]) -> OfDate {
        let mut rotated = [0.0; 3];
        sofars::vm::rxp(&self.rnpb, gcrs, &mut rotated);
        let (theta, phi) = sofars::vm::c2s(&rotated);
        OfDate {
            ra_hours: (theta.to_degrees() / 15.0).rem_euclid(24.0),
            dec_degrees: phi.to_degrees(),
        }
    }

    /// The Sun, geocentric, of date. `None` outside the ephemeris's years.
    pub(crate) fn sun(&self, utc_ms: i64) -> Option<OfDate> {
        let (t1, t2) = tt(utc_ms);
        let (helio, _) = sofars::eph::epv00(t1, t2)?;
        Some(self.of_date(&[-helio[0][0], -helio[0][1], -helio[0][2]]))
    }

    pub(crate) fn sun_altitude(&self, utc_ms: i64) -> Option<f64> {
        let sun = self.sun(utc_ms)?;
        Some(self.horizontal(sun.ra_hours, sun.dec_degrees, utc_ms).0)
    }

    /// The Moon seen from a point on the Earth, of date.
    fn moon_from(&self, utc_ms: i64, place: &[f64; 3]) -> OfDate {
        let (t1, t2) = tt(utc_ms);
        let moon = sofars::eph::moon98(t1, t2)[0];
        let mut of_date = [0.0; 3];
        sofars::vm::rxp(&self.rnpb, &moon, &mut of_date);
        // The place turns with the Earth: its true-equator position at this
        // sidereal time.
        let angle = (self.gast_hours(utc_ms) * 15.0).to_radians();
        let (sin, cos) = angle.sin_cos();
        let x = place[0] * cos - place[1] * sin;
        let y = place[0] * sin + place[1] * cos;
        let topocentric = [
            of_date[0] - x / AU_METERS,
            of_date[1] - y / AU_METERS,
            of_date[2] - place[2] / AU_METERS,
        ];
        let (theta, phi) = sofars::vm::c2s(&topocentric);
        OfDate {
            ra_hours: (theta.to_degrees() / 15.0).rem_euclid(24.0),
            dec_degrees: phi.to_degrees(),
        }
    }

    /// The Moon's topocentric altitude at the site, geometric.
    pub(crate) fn moon_altitude(&self, utc_ms: i64) -> f64 {
        let moon = self.moon_from(utc_ms, &self.observer);
        self.horizontal(moon.ra_hours, moon.dec_degrees, utc_ms).0
    }

    /// The Moon's age in days, 0 at new and about 14.77 at full, from the
    /// difference of the Moon's and the Sun's right ascensions as N.I.N.A.
    /// measures it: for an observer at latitude 0, longitude 0.
    pub(crate) fn moon_age_days(&self, utc_ms: i64) -> Option<f64> {
        let moon = self.moon_from(utc_ms, &self.origin);
        let sun = self.sun(utc_ms)?;
        let mut angle = 15.0 * (moon.ra_hours - sun.ra_hours);
        while angle > 180.0 {
            angle -= 360.0;
        }
        while angle <= -180.0 {
            angle += 360.0;
        }
        let angle = if angle > 0.0 { angle } else { angle + 360.0 };
        Some(angle * LUNAR_CYCLE_DAYS / 360.0)
    }

    /// Degrees between the Moon, seen from the site, and a target's J2000
    /// coordinates brought to the equinox of date.
    pub(crate) fn moon_separation(&self, utc_ms: i64, ra_hours: f64, dec_degrees: f64) -> f64 {
        let moon = self.moon_from(utc_ms, &self.observer);
        separation(moon, self.target_of_date(ra_hours, dec_degrees))
    }

    /// The Moon's altitude and its separation from a target, from one
    /// position of the Moon.
    pub(crate) fn moon_altitude_and_separation(
        &self,
        utc_ms: i64,
        ra_hours: f64,
        dec_degrees: f64,
    ) -> (f64, f64) {
        let moon = self.moon_from(utc_ms, &self.observer);
        let altitude = self.horizontal(moon.ra_hours, moon.dec_degrees, utc_ms).0;
        (
            altitude,
            separation(moon, self.target_of_date(ra_hours, dec_degrees)),
        )
    }

    fn target_of_date(&self, ra_hours: f64, dec_degrees: f64) -> OfDate {
        let target = sofars::vm::s2c((ra_hours * 15.0).to_radians(), dec_degrees.to_radians());
        self.of_date(&target)
    }
}

fn separation(a: OfDate, b: OfDate) -> f64 {
    let (ra1, dec1) = ((a.ra_hours * 15.0).to_radians(), a.dec_degrees.to_radians());
    let (ra2, dec2) = ((b.ra_hours * 15.0).to_radians(), b.dec_degrees.to_radians());
    let cos = dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos();
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

/// N.I.N.A.'s hour angle: sidereal time less right ascension, plus one day
/// once if negative.
pub(crate) fn hour_angle(lst_hours: f64, ra_hours: f64) -> f64 {
    let hour_angle = lst_hours - ra_hours;
    if hour_angle < 0.0 {
        hour_angle + 24.0
    } else {
        hour_angle
    }
}

fn horizontal(
    lst_hours: f64,
    ra_hours: f64,
    dec_degrees: f64,
    latitude_degrees: f64,
) -> (f64, f64) {
    let hour_angle = (hour_angle(lst_hours, ra_hours) * 15.0).to_radians();
    let (dec, lat) = (dec_degrees.to_radians(), latitude_degrees.to_radians());
    let altitude = (dec.sin() * lat.sin() + dec.cos() * lat.cos() * hour_angle.cos())
        .clamp(-1.0, 1.0)
        .asin();
    let cos_azimuth =
        ((dec.sin() - altitude.sin() * lat.sin()) / (altitude.cos() * lat.cos())).clamp(-1.0, 1.0);
    let azimuth = cos_azimuth.acos().to_degrees();
    let azimuth = if hour_angle.sin() < 0.0 {
        azimuth
    } else {
        360.0 - azimuth
    };
    (altitude.to_degrees(), azimuth)
}

/// When the Sun crosses an altitude, from a local noon over the next day,
/// by N.I.N.A.'s method: two-hour windows, a parabola through the altitude
/// at their start, middle and end, and its roots inside the window. Like
/// N.I.N.A., a crossing counts only when the discriminant is at least 1.
/// Returns (set, rise), either missing when it does not happen.
pub(crate) fn sun_crossings(
    frame: &Frame,
    noon_utc_ms: i64,
    threshold_degrees: f64,
) -> (Option<i64>, Option<i64>) {
    let mut rise: Option<i64> = None;
    let mut set: Option<i64> = None;
    let mut offset = 0;
    let altitude = |t: i64| frame.sun_altitude(t).map(|a| a - threshold_degrees);
    loop {
        let start = noon_utc_ms + offset * HOUR_MS;
        let (Some(y0), Some(y1), Some(y2)) = (
            altitude(start),
            altitude(start + HOUR_MS),
            altitude(start + 2 * HOUR_MS),
        ) else {
            return (set, rise);
        };
        let a = 0.5 * (y2 + y0) - y1;
        let b = 2.0 * y1 - 0.5 * y2 - 1.5 * y0;
        let c = y0;
        let discriminant = b * b - 4.0 * a * c;
        let mut first = f64::NAN;
        let mut second = f64::NAN;
        let mut events = 0;
        let inside = |x: f64| (0.0..=2.0).contains(&x);
        if discriminant == 1.0 {
            first = (-b + discriminant.sqrt()) / (2.0 * a);
            if inside(first) {
                events += 1;
            }
        } else if discriminant > 1.0 {
            first = (-b + discriminant.sqrt()) / (2.0 * a);
            second = (-b - discriminant.sqrt()) / (2.0 * a);
            if inside(first) {
                events += 1;
            }
            if inside(second) {
                events += 1;
            }
            if !inside(first) {
                first = second;
            }
        }
        let gradient = 2.0 * a * first + b;
        let at = |x: f64| start + (x * HOUR_MS as f64).round() as i64;
        if events == 1 {
            if gradient > 0.0 {
                rise = Some(at(first));
            } else {
                set = Some(at(first));
            }
        } else if events == 2 {
            if gradient > 0.0 {
                rise = Some(at(first));
                set = Some(at(second));
            } else {
                rise = Some(at(second));
                set = Some(at(first));
            }
        }
        offset += 2;
        if (rise.is_some() && set.is_some()) || offset > 24 {
            return (set, rise);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn los_angeles() -> Site {
        Site {
            latitude_degrees: 34.2,
            longitude_degrees: -118.3,
            elevation_meters: 400.0,
        }
    }

    #[test]
    fn the_sun_sets_and_rises_where_the_core_says() {
        // 2026-09-24, local mean noon at Los Angeles is about 19:53 UTC.
        let noon = 1_790_279_640_000;
        let frame = Frame::new(los_angeles(), noon);
        let (set, rise) = sun_crossings(&frame, noon, -12.0);
        let (set, rise) = (set.unwrap(), rise.unwrap());
        // Nautical darkness falls near 02:40 UTC and ends near 12:55 UTC.
        assert!(
            (set - noon) > 6 * HOUR_MS && (set - noon) < 8 * HOUR_MS,
            "{set}"
        );
        assert!(
            (rise - noon) > 16 * HOUR_MS && (rise - noon) < 18 * HOUR_MS,
            "{rise}"
        );
        // The crossing agrees with a direct altitude to a small fraction of a degree.
        assert!((frame.sun_altitude(set).unwrap() + 12.0).abs() < 0.05);
        let (civil_set, civil_rise) = sun_crossings(&frame, noon, -0.833);
        assert!(civil_set.unwrap() < set && civil_rise.unwrap() > rise);
    }

    #[test]
    fn a_target_transits_near_where_its_hour_angle_is_zero() {
        let noon = 1_790_279_640_000;
        let frame = Frame::new(los_angeles(), noon);
        let ra = 38.2 / 15.0;
        // Local midnight at the start of the 24th, by mean time.
        let midnight = noon - 12 * HOUR_MS;
        let mut transit = frame.transit_after(midnight, ra);
        if transit < noon {
            transit = frame.transit_after(midnight + DAY_MS, ra);
        }
        let hour_angle = hour_angle(frame.lst_hours(transit), ra);
        let off = hour_angle.min(24.0 - hour_angle);
        // Late by up to four minutes of the sidereal-solar slip.
        assert!(off * 60.0 < 4.5, "{off} h");
        let (altitude, azimuth) = frame.horizontal(ra, 61.45, transit);
        assert!(
            (altitude - (90.0 - (61.45_f64 - 34.2).abs())).abs() < 1.0,
            "{altitude}"
        );
        assert!(!(5.0..355.0).contains(&azimuth), "{azimuth}");
    }

    #[test]
    fn the_moon_is_full_late_in_september_2026() {
        let noon = 1_790_279_640_000;
        let frame = Frame::new(los_angeles(), noon);
        let age = frame.moon_age_days(noon + 12 * HOUR_MS).unwrap();
        assert!((age - 14.77).abs() < 2.0, "{age}");
        let separation = frame.moon_separation(noon + 12 * HOUR_MS, 38.2 / 15.0, 61.45);
        assert!(separation > 20.0 && separation < 120.0, "{separation}");
    }
}
