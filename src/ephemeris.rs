//! Where the Sun, Moon and planets are, well enough to mark them on a sky
//! map: the JPL approximate Keplerian elements for 1800–2050 (Standish) for
//! the planets, and a short lunar series (Montenbruck and Pfleger's
//! low-precision Moon). Accuracy is a few arcminutes for the planets and a
//! few arcminutes for the Moon: right for a mark on a framing view, not for
//! pointing a telescope.

use serde::Serialize;

const DEG: f64 = std::f64::consts::PI / 180.0;
/// Mean obliquity of the ecliptic at J2000, degrees.
const OBLIQUITY_J2000: f64 = 23.439_291_1;

/// A body's geocentric J2000 place at one instant.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Placed {
    pub name: &'static str,
    pub kind: BodyKind,
    pub ra_degrees: f64,
    pub dec_degrees: f64,
    /// Distance from Earth in astronomical units.
    pub distance_au: f64,
    /// Elongation from the Sun in degrees; the Moon's phase follows from it.
    pub elongation_degrees: f64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BodyKind {
    Sun,
    Moon,
    Planet,
}

/// Julian date of a Unix time in milliseconds (UTC; the minute of
/// difference from TT does not matter at this accuracy).
pub fn julian_date_from_unix_ms(unix_ms: i64) -> f64 {
    unix_ms as f64 / 86_400_000.0 + 2_440_587.5
}

/// Keplerian elements at J2000 and their rates per Julian century:
/// a (au), e, I (deg), L (deg), long. perihelion (deg), long. node (deg).
struct Elements {
    name: &'static str,
    a: [f64; 2],
    e: [f64; 2],
    i: [f64; 2],
    l: [f64; 2],
    w_bar: [f64; 2],
    node: [f64; 2],
}

const PLANETS: [Elements; 8] = [
    Elements {
        name: "Mercury",
        a: [0.387_099_27, 0.000_000_37],
        e: [0.205_635_93, 0.000_019_06],
        i: [7.004_979_02, -0.005_947_49],
        l: [252.250_323_50, 149_472.674_111_75],
        w_bar: [77.457_796_28, 0.160_476_89],
        node: [48.330_765_93, -0.125_340_81],
    },
    Elements {
        name: "Venus",
        a: [0.723_335_66, 0.000_003_90],
        e: [0.006_776_72, -0.000_041_07],
        i: [3.394_676_05, -0.000_788_90],
        l: [181.979_099_50, 58_517.815_387_29],
        w_bar: [131.602_467_18, 0.002_683_29],
        node: [76.679_842_55, -0.277_694_18],
    },
    Elements {
        name: "Earth",
        a: [1.000_002_61, 0.000_005_62],
        e: [0.016_711_23, -0.000_043_92],
        i: [-0.000_015_31, -0.012_946_68],
        l: [100.464_571_66, 35_999.372_449_81],
        w_bar: [102.937_681_93, 0.323_273_64],
        node: [0.0, 0.0],
    },
    Elements {
        name: "Mars",
        a: [1.523_710_34, 0.000_018_47],
        e: [0.093_394_10, 0.000_078_82],
        i: [1.849_691_42, -0.008_131_31],
        l: [-4.553_432_05, 19_140.302_684_99],
        w_bar: [-23.943_629_59, 0.444_410_88],
        node: [49.559_538_91, -0.292_573_43],
    },
    Elements {
        name: "Jupiter",
        a: [5.202_887_00, -0.000_116_07],
        e: [0.048_386_24, -0.000_132_53],
        i: [1.304_396_95, -0.001_837_14],
        l: [34.396_440_51, 3_034.746_127_75],
        w_bar: [14.728_479_83, 0.212_526_68],
        node: [100.473_909_09, 0.204_691_06],
    },
    Elements {
        name: "Saturn",
        a: [9.536_675_94, -0.001_250_60],
        e: [0.053_861_79, -0.000_509_91],
        i: [2.485_991_87, 0.001_936_09],
        l: [49.954_244_23, 1_222.493_622_01],
        w_bar: [92.598_878_31, -0.418_972_16],
        node: [113.662_424_48, -0.288_677_94],
    },
    Elements {
        name: "Uranus",
        a: [19.189_164_64, -0.001_961_76],
        e: [0.047_257_44, -0.000_043_97],
        i: [0.772_637_83, -0.002_429_39],
        l: [313.238_104_51, 428.482_027_85],
        w_bar: [170.954_276_30, 0.408_052_81],
        node: [74.016_925_03, 0.042_405_89],
    },
    Elements {
        name: "Neptune",
        a: [30.069_922_76, 0.000_262_91],
        e: [0.008_590_48, 0.000_051_05],
        i: [1.770_043_47, 0.000_353_72],
        l: [-55.120_029_69, 218.459_453_25],
        w_bar: [44.964_762_27, -0.322_414_64],
        node: [131.784_225_74, -0.005_086_64],
    },
];

fn at(pair: [f64; 2], centuries: f64) -> f64 {
    pair[0] + pair[1] * centuries
}

/// Heliocentric J2000 ecliptic rectangular coordinates of a planet, au.
fn heliocentric(elements: &Elements, jd: f64) -> [f64; 3] {
    let t = (jd - 2_451_545.0) / 36_525.0;
    let a = at(elements.a, t);
    let e = at(elements.e, t);
    let i = at(elements.i, t) * DEG;
    let l = at(elements.l, t);
    let w_bar = at(elements.w_bar, t);
    let node = at(elements.node, t) * DEG;
    let w = (w_bar - at(elements.node, t)) * DEG;
    let mut m = ((l - w_bar) % 360.0 + 540.0) % 360.0 - 180.0;
    m *= DEG;
    // Kepler's equation, Newton's iteration.
    let mut ecc = m + e * m.sin();
    for _ in 0..20 {
        let delta = (ecc - e * ecc.sin() - m) / (1.0 - e * ecc.cos());
        ecc -= delta;
        if delta.abs() < 1e-12 {
            break;
        }
    }
    let x_orb = a * (ecc.cos() - e);
    let y_orb = a * (1.0 - e * e).sqrt() * ecc.sin();
    let (sin_w, cos_w) = w.sin_cos();
    let (sin_node, cos_node) = node.sin_cos();
    let (sin_i, cos_i) = i.sin_cos();
    [
        (cos_w * cos_node - sin_w * sin_node * cos_i) * x_orb
            + (-sin_w * cos_node - cos_w * sin_node * cos_i) * y_orb,
        (cos_w * sin_node + sin_w * cos_node * cos_i) * x_orb
            + (-sin_w * sin_node + cos_w * cos_node * cos_i) * y_orb,
        (sin_w * sin_i) * x_orb + (cos_w * sin_i) * y_orb,
    ]
}

/// Ecliptic rectangular coordinates to equatorial right ascension and
/// declination in degrees, plus the distance.
fn equatorial(ecliptic: [f64; 3]) -> (f64, f64, f64) {
    let (sin_e, cos_e) = (OBLIQUITY_J2000 * DEG).sin_cos();
    let x = ecliptic[0];
    let y = ecliptic[1] * cos_e - ecliptic[2] * sin_e;
    let z = ecliptic[1] * sin_e + ecliptic[2] * cos_e;
    let r = (x * x + y * y + z * z).sqrt();
    let ra = (y.atan2(x) / DEG).rem_euclid(360.0);
    let dec = (z / r).asin() / DEG;
    (ra, dec, r)
}

fn separation(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let (d1, d2) = (dec1 * DEG, dec2 * DEG);
    let cos = d1.sin() * d2.sin() + d1.cos() * d2.cos() * ((ra1 - ra2) * DEG).cos();
    cos.clamp(-1.0, 1.0).acos() / DEG
}

/// Geocentric J2000 ecliptic coordinates of the Moon, degrees and au.
fn moon_ecliptic(jd: f64) -> (f64, f64, f64) {
    const ARCS: f64 = 206_264.806_247;
    let t = (jd - 2_451_545.0) / 36_525.0;
    let frac = |x: f64| x - x.floor();
    let two_pi = 2.0 * std::f64::consts::PI;
    let l0 = frac(0.606_433 + 1_336.855_225 * t);
    let l = two_pi * frac(0.374_897 + 1_325.552_410 * t);
    let ls = two_pi * frac(0.993_133 + 99.997_361 * t);
    let d = two_pi * frac(0.827_361 + 1_236.853_086 * t);
    let f = two_pi * frac(0.259_086 + 1_342.227_825 * t);
    let dl = 22_640.0 * l.sin() - 4_586.0 * (l - 2.0 * d).sin()
        + 2_370.0 * (2.0 * d).sin()
        + 769.0 * (2.0 * l).sin()
        - 668.0 * ls.sin()
        - 412.0 * (2.0 * f).sin()
        - 212.0 * (2.0 * l - 2.0 * d).sin()
        - 206.0 * (l + ls - 2.0 * d).sin()
        + 192.0 * (l + 2.0 * d).sin()
        - 165.0 * (ls - 2.0 * d).sin()
        - 125.0 * d.sin()
        - 110.0 * (l + ls).sin()
        + 148.0 * (l - ls).sin()
        - 55.0 * (2.0 * f - 2.0 * d).sin();
    let s = f + (dl + 412.0 * (2.0 * f).sin() + 541.0 * ls.sin()) / ARCS;
    let h = f - 2.0 * d;
    let n = -526.0 * h.sin() + 44.0 * (l + h).sin() - 31.0 * (-l + h).sin() - 23.0 * (ls + h).sin()
        + 11.0 * (-ls + h).sin()
        - 25.0 * (-2.0 * l + f).sin()
        + 21.0 * (-l + f).sin();
    let lon = two_pi * frac(l0 + dl / 1_296_000.0);
    let lat = (18_520.0 * s.sin() + n) / ARCS;
    // Distance from the parallax, a short series too (Meeus-style terms).
    let parallax_arcsec = 3_422.7
        + 186.5 * l.cos()
        + 34.3 * (l - 2.0 * d).cos()
        + 28.2 * (2.0 * d).cos()
        + 10.2 * (2.0 * l).cos();
    let distance_km = 6_378.137 / (parallax_arcsec / ARCS).sin();
    (lon / DEG, lat / DEG, distance_km / 149_597_870.7)
}

/// The Sun, the Moon and the eight planets seen from Earth at `jd`.
pub fn solar_system_at(jd: f64) -> Vec<Placed> {
    let earth = heliocentric(&PLANETS[2], jd);
    let (sun_ra, sun_dec, sun_distance) = equatorial([-earth[0], -earth[1], -earth[2]]);
    let mut out = vec![Placed {
        name: "Sun",
        kind: BodyKind::Sun,
        ra_degrees: sun_ra,
        dec_degrees: sun_dec,
        distance_au: sun_distance,
        elongation_degrees: 0.0,
    }];
    let (moon_lon, moon_lat, moon_distance) = moon_ecliptic(jd);
    let (lon, lat) = (moon_lon * DEG, moon_lat * DEG);
    let (moon_ra, moon_dec, _) =
        equatorial([lat.cos() * lon.cos(), lat.cos() * lon.sin(), lat.sin()]);
    out.push(Placed {
        name: "Moon",
        kind: BodyKind::Moon,
        ra_degrees: moon_ra,
        dec_degrees: moon_dec,
        distance_au: moon_distance,
        elongation_degrees: separation(moon_ra, moon_dec, sun_ra, sun_dec),
    });
    for elements in PLANETS.iter().filter(|p| p.name != "Earth") {
        let planet = heliocentric(elements, jd);
        // One light-time step is plenty at arcminute accuracy.
        let geocentric = [
            planet[0] - earth[0],
            planet[1] - earth[1],
            planet[2] - earth[2],
        ];
        let (ra, dec, distance) = equatorial(geocentric);
        out.push(Placed {
            name: elements.name,
            kind: BodyKind::Planet,
            ra_degrees: ra,
            dec_degrees: dec,
            distance_au: distance,
            elongation_degrees: separation(ra, dec, sun_ra, sun_dec),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn jd(year: i32, month: u32, day: f64) -> f64 {
        // Meeus 7.1, Gregorian calendar.
        let (y, m) = if month <= 2 {
            (year - 1, month + 12)
        } else {
            (year, month)
        };
        let a = (y as f64 / 100.0).floor();
        let b = 2.0 - a + (a / 4.0).floor();
        (365.25 * (y as f64 + 4716.0)).floor() + (30.6001 * (m as f64 + 1.0)).floor() + day + b
            - 1524.5
    }

    fn body<'a>(placed: &'a [Placed], name: &str) -> &'a Placed {
        placed.iter().find(|p| p.name == name).unwrap()
    }

    #[test]
    fn the_sun_and_planets_at_j2000_land_where_the_almanac_puts_them() {
        let placed = solar_system_at(jd(2000, 1, 1.5));
        let close = |name: &str, ra: f64, dec: f64, within: f64| {
            let p = body(&placed, name);
            let off = separation(p.ra_degrees, p.dec_degrees, ra, dec);
            assert!(
                off < within,
                "{name}: {:.2},{:.2} is {off:.2}° from {ra},{dec}",
                p.ra_degrees,
                p.dec_degrees
            );
        };
        // Apparent places for 2000-01-01 12:00 TT, from the Astronomical Almanac.
        close("Sun", 281.29, -23.03, 0.5);
        close("Jupiter", 23.9, 8.6, 2.0);
        close("Saturn", 38.8, 12.4, 2.0);
        close("Mars", 330.2, -13.2, 2.0);
        close("Venus", 239.4, -18.3, 2.0);
        assert!((body(&placed, "Sun").distance_au - 0.983).abs() < 0.01);
    }

    #[test]
    fn the_moon_is_new_and_full_when_the_calendar_says_so() {
        // New Moon 2000-01-06 18:14 UT; Full Moon 2000-01-21 04:40 UT (the eclipse night).
        let new = solar_system_at(jd(2000, 1, 6.76));
        assert!(
            body(&new, "Moon").elongation_degrees < 5.0,
            "{:?}",
            body(&new, "Moon")
        );
        let full = solar_system_at(jd(2000, 1, 21.19));
        assert!(
            body(&full, "Moon").elongation_degrees > 175.0,
            "{:?}",
            body(&full, "Moon")
        );
        let moon = body(&full, "Moon");
        assert!(
            moon.distance_au > 0.0023 && moon.distance_au < 0.0028,
            "{moon:?}"
        );
    }

    #[test]
    fn unix_time_converts_to_julian_date() {
        assert!((julian_date_from_unix_ms(0) - 2_440_587.5).abs() < 1e-9);
        assert!((julian_date_from_unix_ms(946_728_000_000) - 2_451_545.0).abs() < 1e-9);
    }
}
