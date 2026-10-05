//! N.I.N.A. horizon files (`.hrz`): one `azimuth altitude` pair per line in
//! degrees, `#` comments, spaces or tabs between columns. A file need not
//! start at 0° or end at 360°; the curve wraps, so the missing endpoints are
//! interpolated across north. Output is the canonical form `Horizon` checks.

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
    /// The same azimuth twice, which leaves the curve undefined there.
    RepeatedAzimuth {
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
            Self::RepeatedAzimuth { line } => {
                write!(f, "line {line} repeats an azimuth from an earlier line")
            }
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
        let mut points: Vec<(HorizonPoint, usize)> = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let content = raw.split('#').next().unwrap_or("").trim();
            if content.is_empty() {
                continue;
            }
            let columns: Vec<&str> = content.split_whitespace().collect();
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
            points.push((
                HorizonPoint {
                    azimuth_degrees: azimuth,
                    altitude_degrees: altitude,
                },
                line,
            ));
        }
        if points.is_empty() {
            return Err(HrzError::Empty);
        }
        points.sort_by(|a, b| a.0.azimuth_degrees.total_cmp(&b.0.azimuth_degrees));
        if let Some(pair) = points
            .windows(2)
            .find(|pair| pair[0].0.azimuth_degrees == pair[1].0.azimuth_degrees)
        {
            return Err(HrzError::RepeatedAzimuth {
                line: pair[0].1.max(pair[1].1),
            });
        }
        let mut points: Vec<HorizonPoint> = points.into_iter().map(|(point, _)| point).collect();
        let first = points[0];
        let last = points[points.len() - 1];
        // The altitude where the curve crosses north, between the last point
        // and the first one carried round by a full turn.
        let span = first.azimuth_degrees + 360.0 - last.azimuth_degrees;
        let north = if span > 0.0 && span < 360.0 {
            last.altitude_degrees
                + (360.0 - last.azimuth_degrees) / span
                    * (first.altitude_degrees - last.altitude_degrees)
        } else {
            first.altitude_degrees
        };
        if first.azimuth_degrees != 0.0 {
            let at_zero = if last.azimuth_degrees == 360.0 {
                last.altitude_degrees
            } else {
                north
            };
            points.insert(
                0,
                HorizonPoint {
                    azimuth_degrees: 0.0,
                    altitude_degrees: at_zero,
                },
            );
        }
        if last.azimuth_degrees != 360.0 {
            let at_full_turn = if first.azimuth_degrees == 0.0 {
                first.altitude_degrees
            } else {
                north
            };
            points.push(HorizonPoint {
                azimuth_degrees: 360.0,
                altitude_degrees: at_full_turn,
            });
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
    fn missing_endpoints_are_interpolated_across_north() {
        // 350° at 20 and 10° at 40: north sits halfway, at 30.
        let horizon = Horizon::from_hrz("10 40\n180 15\n350 20\n").unwrap();
        assert_eq!(
            points(&horizon),
            vec![
                (0.0, 30.0),
                (10.0, 40.0),
                (180.0, 15.0),
                (350.0, 20.0),
                (360.0, 30.0)
            ]
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
        assert_eq!(
            Horizon::from_hrz("90 10\n0 3\n90 4\n"),
            Err(HrzError::RepeatedAzimuth { line: 3 })
        );
        assert_eq!(Horizon::from_hrz("# nothing\n\n"), Err(HrzError::Empty));
    }

    #[test]
    fn writing_and_reading_round_trips() {
        let horizon = Horizon::from_hrz("0 10.25\n45 18\n360 11\n").unwrap();
        let text = horizon.to_hrz().unwrap();
        assert_eq!(Horizon::from_hrz(&text).unwrap(), horizon);
        assert_eq!(Horizon::FixedMinimum {}.to_hrz(), None);
    }
}
