//! N.I.N.A. horizon files (`.hrz`): one `azimuth altitude` pair per line in
//! degrees, `#` comments, and spaces, tabs, commas or semicolons between
//! the columns. An azimuth given on more than one line, as a file may give to
//! draw the upright edge of a building, takes the highest of its altitudes:
//! the most restrictive reading. N.I.N.A. takes the last line's instead, so
//! at that one azimuth PSF Guard may plan more strictly than N.I.N.A.
//! Otherwise the points are read as N.I.N.A.'s `CustomHorizon` reads them,
//! so PSF Guard plans against the horizon Target Scheduler uses:
//!
//! - a file with 0° but not 360°, or 360° but not 0°, gives the missing end
//!   the other's altitude;
//! - a file with neither gives both the altitude of the point nearest
//!   north, the first one when the two are as near.
//!
//! Output is the canonical form `Horizon` checks.

use super::{Horizon, HorizonPoint};
use std::fmt::Write as _;

/// Why a file was refused, with the 1-based line where it applies.
#[derive(Clone, Debug, PartialEq)]
pub enum HrzError {
    /// A line that is neither a comment, blank, nor two numbers.
    Unreadable {
        line: usize,
    },
    /// An azimuth outside 0..=360 or an altitude outside -90..=90.
    OutOfRange {
        line: usize,
    },
    Empty,
    TooManyPoints,
}

impl std::fmt::Display for HrzError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreadable { line } => write!(f, "line {line} is not an azimuth and an altitude"),
            Self::OutOfRange { line } => write!(
                f,
                "line {line} is out of range: azimuth runs 0 to 360, altitude -90 to 90"
            ),
            Self::Empty => write!(f, "the file has no horizon points"),
            Self::TooManyPoints => write!(f, "the file has more than {MAX_POINTS} points"),
        }
    }
}

impl std::error::Error for HrzError {}

/// Room for the two endpoints the parser may add within the core's bound.
const MAX_POINTS: usize = 4096;

impl Horizon {
    pub fn from_hrz(text: &str) -> Result<Self, HrzError> {
        let mut points: Vec<HorizonPoint> = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let content = raw.split('#').next().unwrap_or("").trim();
            if content.is_empty() {
                continue;
            }
            let columns: Vec<&str> = content
                .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
                .filter(|column| !column.is_empty())
                .collect();
            let [azimuth, altitude] = columns[..] else {
                return Err(HrzError::Unreadable { line });
            };
            let (Ok(azimuth), Ok(altitude)) = (azimuth.parse::<f64>(), altitude.parse::<f64>())
            else {
                return Err(HrzError::Unreadable { line });
            };
            if !azimuth.is_finite()
                || !altitude.is_finite()
                || !(0.0..=360.0).contains(&azimuth)
                || !(-90.0..=90.0).contains(&altitude)
            {
                return Err(HrzError::OutOfRange { line });
            }
            if points.len() == MAX_POINTS {
                return Err(HrzError::TooManyPoints);
            }
            points.push(HorizonPoint {
                // -0 is 0, as N.I.N.A.'s map keys compare it.
                azimuth_degrees: azimuth + 0.0,
                altitude_degrees: altitude,
            });
        }
        if points.is_empty() {
            return Err(HrzError::Empty);
        }
        // An azimuth given again keeps the highest altitude: a target must
        // clear every line the file draws there.
        points.sort_by(|a, b| a.azimuth_degrees.total_cmp(&b.azimuth_degrees));
        points.dedup_by(|later, kept| {
            let same = later.azimuth_degrees == kept.azimuth_degrees;
            if same {
                kept.altitude_degrees = kept.altitude_degrees.max(later.altitude_degrees);
            }
            same
        });
        let first = points[0];
        let last = points[points.len() - 1];
        let end = |altitude_degrees| HorizonPoint {
            azimuth_degrees: 0.0,
            altitude_degrees,
        };
        match (first.azimuth_degrees == 0.0, last.azimuth_degrees == 360.0) {
            (true, true) => {}
            (true, false) => points.push(HorizonPoint {
                azimuth_degrees: 360.0,
                ..first
            }),
            (false, true) => points.insert(0, end(last.altitude_degrees)),
            (false, false) => {
                let nearest = if 360.0 - last.azimuth_degrees < first.azimuth_degrees {
                    last
                } else {
                    first
                };
                points.insert(0, end(nearest.altitude_degrees));
                points.push(HorizonPoint {
                    azimuth_degrees: 360.0,
                    ..nearest
                });
            }
        }
        let horizon = Self::Custom { points };
        horizon.validate().map_err(|_| HrzError::TooManyPoints)?;
        Ok(horizon)
    }

    /// The file N.I.N.A. reads back. A flat horizon has no curve to write.
    pub fn to_hrz(&self) -> Option<String> {
        let Self::Custom { points } = self else {
            return None;
        };
        let mut text = String::from("# Azimuth Altitude, degrees\n");
        for point in points {
            let _ = writeln!(text, "{} {}", point.azimuth_degrees, point.altitude_degrees);
        }
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(horizon: &Horizon) -> Vec<(f64, f64)> {
        match horizon {
            Horizon::Custom { points } => points
                .iter()
                .map(|p| (p.azimuth_degrees, p.altitude_degrees))
                .collect(),
            Horizon::FixedMinimum {} => vec![],
        }
    }

    #[test]
    fn a_full_file_reads_as_written() {
        let horizon =
            Horizon::from_hrz("# from N.I.N.A.\n0 10\n90\t25.5\n\n180 30 # trees\n360 12\n")
                .unwrap();
        assert_eq!(
            points(&horizon),
            vec![(0.0, 10.0), (90.0, 25.5), (180.0, 30.0), (360.0, 12.0)]
        );
    }

    #[test]
    fn missing_endpoints_take_the_point_nearest_north_as_nina_does() {
        // 10° and 350° are as near north: N.I.N.A. takes the first.
        let horizon = Horizon::from_hrz("10 40\n180 15\n350 20\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![
                (0.0, 40.0),
                (10.0, 40.0),
                (180.0, 15.0),
                (350.0, 20.0),
                (360.0, 40.0)
            ]
        );
        // The reporter's file (#1126): 1.79° is 1.79° from north and
        // 357.49° is 2.51°, so both ends take 1.79°'s 8.52.
        let horizon = Horizon::from_hrz("1.79 8.52\n180 30\n357.49 17.16\n").unwrap();
        assert_eq!(points(&horizon)[0], (0.0, 8.52));
        assert_eq!(points(&horizon)[4], (360.0, 8.52));
        let horizon = Horizon::from_hrz("5 12\n180 30\n358 25\n").unwrap();
        assert_eq!(points(&horizon)[0], (0.0, 25.0));
        assert_eq!(points(&horizon)[4], (360.0, 25.0));
    }

    #[test]
    fn columns_may_be_split_by_commas_semicolons_or_tabs_and_minus_zero_is_zero() {
        let horizon = Horizon::from_hrz("0,10\n90;20\n180\t30\n-0 5\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![(0.0, 10.0), (90.0, 20.0), (180.0, 30.0), (360.0, 10.0)]
        );
    }

    #[test]
    fn one_present_endpoint_supplies_the_other() {
        let horizon = Horizon::from_hrz("0 15\n200 25\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![(0.0, 15.0), (200.0, 25.0), (360.0, 15.0)]
        );
        let horizon = Horizon::from_hrz("200 25\n360 18\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![(0.0, 18.0), (200.0, 25.0), (360.0, 18.0)]
        );
    }

    #[test]
    fn a_single_point_is_a_flat_curve() {
        let horizon = Horizon::from_hrz("120 22\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![(0.0, 22.0), (120.0, 22.0), (360.0, 22.0)]
        );
    }

    #[test]
    fn unsorted_lines_are_ordered() {
        let horizon = Horizon::from_hrz("270 5\n90 8\n0 1\n360 2\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![(0.0, 1.0), (90.0, 8.0), (270.0, 5.0), (360.0, 2.0)]
        );
    }

    #[test]
    fn bad_lines_are_named() {
        assert_eq!(
            Horizon::from_hrz("0 10\nnorth 12\n"),
            Err(HrzError::Unreadable { line: 2 })
        );
        assert_eq!(
            Horizon::from_hrz("0 10 3\n"),
            Err(HrzError::Unreadable { line: 1 })
        );
        assert_eq!(
            Horizon::from_hrz("0 10\n\n400 3\n"),
            Err(HrzError::OutOfRange { line: 3 })
        );
        assert_eq!(
            Horizon::from_hrz("0 10\n90 95\n"),
            Err(HrzError::OutOfRange { line: 2 })
        );
        assert_eq!(Horizon::from_hrz("# nothing\n\n"), Err(HrzError::Empty));
    }

    #[test]
    fn a_repeated_azimuth_keeps_its_highest_altitude() {
        // The upright edge of a building at 321.74°, drawn as three lines,
        // and another at 322.74° (#1126). The highest altitude stands.
        let horizon = Horizon::from_hrz(
            "0 10\n320.74 7.45\n321.74 8.45\n321.74 12.45\n321.74 21.45\n\
             322.74 11.45\n322.74 14.45\n325.74 25.45\n360 10\n",
        )
        .unwrap();
        assert_eq!(
            points(&horizon),
            vec![
                (0.0, 10.0),
                (320.74, 7.45),
                (321.74, 21.45),
                (322.74, 14.45),
                (325.74, 25.45),
                (360.0, 10.0)
            ]
        );
        horizon.validate().unwrap();
        // The highest, wherever its line falls; N.I.N.A. would take 4.
        assert_eq!(
            points(&Horizon::from_hrz("90 10\n0 3\n90 4\n").unwrap()),
            vec![(0.0, 3.0), (90.0, 10.0), (360.0, 3.0)]
        );
    }

    #[test]
    fn writing_and_reading_round_trips() {
        let horizon = Horizon::from_hrz("0 10.25\n45 18\n360 11\n").unwrap();
        let text = horizon.to_hrz().unwrap();
        assert_eq!(Horizon::from_hrz(&text).unwrap(), horizon);
        assert_eq!(Horizon::FixedMinimum {}.to_hrz(), None);
    }
}
