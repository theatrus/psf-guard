//! Conservative whole-span altitude screening for the fixed-star SOFA model.

use super::{
    altitude_allowed, observe, AltitudeLimits, EarthOrientation, Horizon, IcrsPosition,
    ObservedPosition, Site, VisibilityError,
};
use crate::windows::Interval;
use chrono::Datelike;
use serde::{Deserialize, Serialize};

// Model envelope, not a user-adjustable mount tracking limit. Earth rotation is
// <0.0042 deg/s; 0.01 also covers the much slower apparent-star terms in the
// pinned zero-pressure, zero-proper-motion terrestrial model. The angular guard
// covers numeric error. UTC offset changes are rejected separately. See design doc.
const MOTION_DEGREES_PER_SECOND: f64 = 0.01;
const ANGULAR_GUARD_DEGREES: f64 = 0.001;
const MAX_SPAN_MS: u64 = 86_400_000;
const MAX_OBSERVATIONS: usize = 8192;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum AltitudeSpan {
    Clear,
    /// A sampled point demonstrably violates the altitude/horizon constraints.
    Blocked {
        at_ms: u64,
    },
}

/// Screen the entire closed span, including its finish instant. The caller must
/// include exposure AND blocking overhead, and rerun after preparation delays.
/// Only Clear proves altitude clearance within this fixed-star model envelope.
/// UnresolvedSpan and SpanBudgetExceeded are unknown, never permission to run.
/// This does not check darkness, meridian policy, assignment, weather, equipment,
/// or dispatch ownership. It must not be used as a complete hardware permit.
pub fn check_altitude_span(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    horizon: &Horizon,
    limits: AltitudeLimits,
    span: Interval,
) -> Result<AltitudeSpan, VisibilityError> {
    check_span(
        position,
        site,
        orientation,
        horizon,
        limits,
        span,
        MAX_OBSERVATIONS,
    )
}

fn check_span(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    horizon: &Horizon,
    limits: AltitudeLimits,
    span: Interval,
    budget: usize,
) -> Result<AltitudeSpan, VisibilityError> {
    if span.start_ms >= span.end_ms || span.end_ms - span.start_ms > MAX_SPAN_MS {
        return Err(VisibilityError::InvalidSpan);
    }
    // Validate both endpoints before reporting even a blocked start: stale or
    // unsupported end evidence means this is not a valid whole-span request.
    let first = observe(position, site, orientation, span.start_ms)?;
    let last = observe(position, site, orientation, span.end_ms)?;
    if tai_minus_utc_day(span.start_ms)? != tai_minus_utc_day(span.end_ms)? {
        // A single constant DUT1 cannot model an offset adjustment. The caller
        // cannot repair this by extending the evidence's declared validity.
        return Err(VisibilityError::EarthOrientationDiscontinuity);
    }
    for (at_ms, point) in [(span.start_ms, first), (span.end_ms, last)] {
        if !altitude_allowed(
            horizon,
            limits,
            point.azimuth_degrees,
            point.altitude_degrees,
        )? {
            return Ok(AltitudeSpan::Blocked { at_ms });
        }
    }
    let mut observations = 2;
    let mut pending = vec![span];
    while let Some(part) = pending.pop() {
        if observations >= budget {
            return Err(VisibilityError::SpanBudgetExceeded);
        }
        let midpoint = part.start_ms + (part.end_ms - part.start_ms) / 2;
        let point = observe(position, site, orientation, midpoint)?;
        observations += 1;
        if !altitude_allowed(
            horizon,
            limits,
            point.azimuth_degrees,
            point.altitude_degrees,
        )? {
            return Ok(AltitudeSpan::Blocked { at_ms: midpoint });
        }
        // ceil half-span also covers odd millisecond lengths and both endpoints.
        let radius = (part.end_ms - midpoint) as f64 / 1000.0 * MOTION_DEGREES_PER_SECOND
            + ANGULAR_GUARD_DEGREES;
        if cap_clear(horizon, limits, point, radius)? {
            continue;
        }
        if part.end_ms - part.start_ms <= 1 {
            return Err(VisibilityError::UnresolvedSpan);
        }
        // Depth-first subdivision bounds memory by log2(MAX_SPAN_MS), not by
        // elapsed time or horizon point count. Visit earlier times first.
        pending.push(Interval {
            start_ms: midpoint,
            end_ms: part.end_ms,
        });
        pending.push(Interval {
            start_ms: part.start_ms,
            end_ms: midpoint,
        });
    }
    Ok(AltitudeSpan::Clear)
}

fn tai_minus_utc_day(unix_ms: u64) -> Result<f64, VisibilityError> {
    let millis = i64::try_from(unix_ms).map_err(|_| VisibilityError::UnsupportedTime)?;
    let time =
        chrono::DateTime::from_timestamp_millis(millis).ok_or(VisibilityError::UnsupportedTime)?;
    sofars::ts::dat(time.year(), time.month() as i32, time.day() as i32, 0.0)
        .map_err(|_| VisibilityError::AstronomyUnavailable)
}

fn cap_clear(
    horizon: &Horizon,
    limits: AltitudeLimits,
    center: ObservedPosition,
    radius: f64,
) -> Result<bool, VisibilityError> {
    let lower = center.altitude_degrees - radius;
    let upper = center.altitude_degrees + radius;
    if lower
        <= limits
            .rig_minimum_degrees
            .max(limits.project_minimum_degrees)
        || upper
            >= limits
                .rig_maximum_degrees
                .min(limits.project_maximum_degrees)
    {
        return Ok(false);
    }
    let Horizon::Custom { points } = horizon else {
        return Ok(true);
    };
    // Spherical metric ds^2 = dh^2 + cos(h)^2 da^2. On this cap, |h| is
    // bounded by |center altitude| + radius. Integrating gives |da| <= r/cos(h).
    // A cap touching either pole can span every azimuth; never divide near zero.
    let maximum_latitude = center.altitude_degrees.abs() + radius;
    let azimuth_radius = if maximum_latitude >= 90.0 {
        180.0
    } else {
        (radius / maximum_latitude.to_radians().cos()).min(180.0)
    };
    let maximum = if azimuth_radius >= 180.0 {
        points
            .iter()
            .map(|p| p.altitude_degrees)
            .fold(-90.0, f64::max)
    } else {
        let azimuth = center.azimuth_degrees.rem_euclid(360.0);
        let start = azimuth - azimuth_radius;
        let end = azimuth + azimuth_radius;
        let mut maximum = horizon
            .altitude(start)?
            .unwrap()
            .max(horizon.altitude(end)?.unwrap());
        // A linear curve reaches its extrema at endpoints or knots. Include ALL
        // knots in the swept arc, including both sides of the north discontinuity.
        for point in points {
            if [-360.0, 0.0, 360.0]
                .iter()
                .any(|offset| (start..=end).contains(&(point.azimuth_degrees + offset)))
            {
                maximum = maximum.max(point.altitude_degrees);
            }
        }
        maximum
    };
    Ok(lower > maximum + limits.horizon_offset_degrees)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::visibility::HorizonPoint;

    #[test]
    fn exhausting_observation_budget_never_returns_clear() {
        let start = 1_790_409_600_000;
        let result = check_span(
            IcrsPosition {
                ra_degrees: 83.0,
                dec_degrees: -5.0,
            },
            Site {
                latitude_degrees: 35.0,
                longitude_degrees: -120.0,
                elevation_meters: 1000.0,
            },
            EarthOrientation {
                ut1_minus_utc_seconds: 0.0,
                polar_motion_x_radians: 0.0,
                polar_motion_y_radians: 0.0,
                valid_from_ms: start,
                valid_until_ms: start + 1001,
            },
            &Horizon::FixedMinimum {},
            AltitudeLimits {
                rig_minimum_degrees: -89.0,
                project_minimum_degrees: -89.0,
                horizon_offset_degrees: 0.0,
                rig_maximum_degrees: 89.0,
                project_maximum_degrees: 89.0,
            },
            Interval {
                start_ms: start,
                end_ms: start + 1000,
            },
            2,
        );
        assert_eq!(result, Err(VisibilityError::SpanBudgetExceeded));
    }

    #[test]
    fn cap_does_not_skip_sub_sample_knots_or_north_discontinuity() {
        let limits = AltitudeLimits {
            rig_minimum_degrees: -90.0,
            project_minimum_degrees: -90.0,
            rig_maximum_degrees: 90.0,
            project_maximum_degrees: 90.0,
            horizon_offset_degrees: 0.0,
        };
        for azimuth in [0.0_f64, 100.0, 359.999] {
            let peak = azimuth + 0.0001;
            let horizon = Horizon::Custom {
                points: vec![
                    HorizonPoint {
                        azimuth_degrees: 0.0,
                        altitude_degrees: -80.0,
                    },
                    HorizonPoint {
                        azimuth_degrees: peak,
                        altitude_degrees: 80.0,
                    },
                    HorizonPoint {
                        azimuth_degrees: peak.next_up(),
                        altitude_degrees: -80.0,
                    },
                    HorizonPoint {
                        azimuth_degrees: 360.0,
                        altitude_degrees: -80.0,
                    },
                ],
            };
            let point = ObservedPosition {
                azimuth_degrees: azimuth,
                altitude_degrees: 40.0,
                hour_angle_degrees: 0.0,
            };
            assert!(!cap_clear(&horizon, limits, point, 0.001).unwrap());
        }
        let horizon = Horizon::Custom {
            points: vec![
                HorizonPoint {
                    azimuth_degrees: 0.0,
                    altitude_degrees: -80.0,
                },
                HorizonPoint {
                    azimuth_degrees: 360.0,
                    altitude_degrees: 80.0,
                },
            ],
        };
        let point = ObservedPosition {
            azimuth_degrees: 0.0,
            altitude_degrees: 40.0,
            hour_angle_degrees: 0.0,
        };
        assert!(!cap_clear(&horizon, limits, point, 0.001).unwrap());
    }
}
