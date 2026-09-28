//! Sun and Moon for planning: where they are, how bright the Moon is, and
//! how far it sits from a target. Planning grade, from SOFA's simplified
//! Earth ephemeris and Meeus-based lunar theory; not an almanac, and never
//! a substitute for the rig's own darkness or safety evidence.

use crate::visibility::IcrsPosition;

const UNIX_EPOCH_JD: f64 = 2_440_587.5;
/// TT minus UTC since 2017, seconds. Planning positions move less than an
/// arcsecond over the leap seconds this ignores.
const TT_MINUS_UTC_SECONDS: f64 = 69.184;

fn tt_two_part(unix_ms: u64) -> (f64, f64) {
    let days = unix_ms as f64 / 86_400_000.0;
    (UNIX_EPOCH_JD, days + TT_MINUS_UTC_SECONDS / 86_400.0)
}

fn to_position(vector: &[f64; 3]) -> IcrsPosition {
    let (theta, phi) = sofars::vm::c2s(vector);
    IcrsPosition {
        ra_degrees: theta.to_degrees().rem_euclid(360.0),
        dec_degrees: phi.to_degrees().clamp(-90.0, 90.0),
    }
}

fn norm(v: &[f64; 3]) -> f64 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

/// Geocentric Sun direction, ICRS. Aberration and light time (under half an
/// arcminute together) are left out.
pub fn sun_position(unix_ms: u64) -> Option<IcrsPosition> {
    let (d1, d2) = tt_two_part(unix_ms);
    let (helio, _) = sofars::eph::epv00(d1, d2)?;
    let sun = [-helio[0][0], -helio[0][1], -helio[0][2]];
    Some(to_position(&sun))
}

/// Geocentric Moon direction, ICRS. Topocentric parallax (up to a degree)
/// is left out; a planner comparing Moon separations does not need it.
pub fn moon_position(unix_ms: u64) -> IcrsPosition {
    let (d1, d2) = tt_two_part(unix_ms);
    let pv = sofars::eph::moon98(d1, d2);
    to_position(&pv[0])
}

/// Illuminated fraction of the Moon's disc, 0 (new) to 1 (full).
pub fn moon_illumination(unix_ms: u64) -> Option<f64> {
    let (d1, d2) = tt_two_part(unix_ms);
    let (helio, _) = sofars::eph::epv00(d1, d2)?;
    let sun = [-helio[0][0], -helio[0][1], -helio[0][2]];
    let moon = sofars::eph::moon98(d1, d2)[0];
    let (r_sun, r_moon) = (norm(&sun), norm(&moon));
    if r_sun == 0.0 || r_moon == 0.0 {
        return None;
    }
    let cos_elongation = ((sun[0] * moon[0] + sun[1] * moon[1] + sun[2] * moon[2])
        / (r_sun * r_moon))
        .clamp(-1.0, 1.0);
    let elongation = cos_elongation.acos();
    // Meeus 48.3: the phase angle seen from the Moon.
    let phase = (r_sun * elongation.sin()).atan2(r_moon - r_sun * elongation.cos());
    Some(((1.0 + phase.cos()) / 2.0).clamp(0.0, 1.0))
}

/// Great-circle separation in degrees.
pub fn separation_degrees(a: IcrsPosition, b: IcrsPosition) -> f64 {
    let (ra1, dec1) = (a.ra_degrees.to_radians(), a.dec_degrees.to_radians());
    let (ra2, dec2) = (b.ra_degrees.to_radians(), b.dec_degrees.to_radians());
    let cos = dec1.sin() * dec2.sin() + dec1.cos() * dec2.cos() * (ra1 - ra2).cos();
    cos.clamp(-1.0, 1.0).acos().to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn ms(unix_seconds: u64) -> u64 {
        unix_seconds * 1000
    }

    #[test]
    fn the_sun_crosses_the_equator_at_the_equinox_and_peaks_at_the_solstice() {
        // 2026-03-20 14:46 UTC and 2026-06-21 08:24 UTC.
        // Positions are ICRS, so at the equinox of date the Sun sits about
        // 0.36 degrees short of the J2000 equinox: 26 years of precession.
        let equinox = sun_position(ms(1_774_017_960)).unwrap();
        assert!((equinox.dec_degrees + 0.144).abs() < 0.03, "{equinox:?}");
        assert!(
            equinox.ra_degrees > 359.5 && equinox.ra_degrees < 359.8,
            "{equinox:?}"
        );
        let solstice = sun_position(ms(1_782_030_240)).unwrap();
        assert!((solstice.dec_degrees - 23.44).abs() < 0.05, "{solstice:?}");
        assert!((solstice.ra_degrees - 89.6).abs() < 0.3, "{solstice:?}");
    }

    #[test]
    fn the_moon_is_full_on_2026_09_26_and_new_on_2026_09_11() {
        // Full 2026-09-26 16:49 UTC; new 2026-09-11 03:27 UTC.
        assert!(moon_illumination(ms(1_790_441_340)).unwrap() > 0.99);
        assert!(moon_illumination(ms(1_789_097_220)).unwrap() < 0.01);
        // A full Moon stands opposite the Sun.
        let sun = sun_position(ms(1_790_441_340)).unwrap();
        let moon = moon_position(ms(1_790_441_340));
        let apart = separation_degrees(sun, moon);
        assert!(apart > 174.0, "{apart}");
    }

    #[test]
    fn separations_are_symmetric_and_bounded() {
        let a = IcrsPosition {
            ra_degrees: 10.0,
            dec_degrees: 41.0,
        };
        let b = IcrsPosition {
            ra_degrees: 190.0,
            dec_degrees: -41.0,
        };
        assert!((separation_degrees(a, b) - 180.0).abs() < 1e-9);
        assert_eq!(separation_degrees(a, a), 0.0);
        let c = IcrsPosition {
            ra_degrees: 11.0,
            dec_degrees: 41.0,
        };
        assert!((separation_degrees(a, c) - separation_degrees(c, a)).abs() < 1e-12);
        assert!(separation_degrees(a, c) < 1.0 && separation_degrees(a, c) > 0.7);
    }
}
