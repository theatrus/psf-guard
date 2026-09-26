use super::span::{
    classify_cap, midpoint_and_radius, validate_span, CapClearance, MAX_OBSERVATIONS,
};
use super::{
    altitude_allowed, observe, AltitudeLimits, EarthOrientation, Horizon, IcrsPosition, Site,
    VisibilityError,
};
use crate::windows::{Interval, MAX_WINDOWS};
use serde::{Deserialize, Serialize};

const RESOLUTION_MS: u64 = 1000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AltitudeWindows {
    pub searched: Interval,
    /// Conservative windows only. Every instant, including each finish, cleared
    /// the same model envelope used by check_altitude_span.
    pub windows: Vec<Interval>,
    /// Regions not resolved at one-second granularity. Never eligible windows.
    /// Empty windows with nonempty unresolved is not proof of total obstruction.
    pub unresolved: Vec<Interval>,
}

/// Construct conservative altitude windows over at most 24 hours. Fully clear
/// or blocked spherical caps prune the search; uncertain caps subdivide. Gaps
/// remain gaps, even when no timestamp sample lands on the obstruction itself.
/// No partial result is returned on budget, count, validity, or model errors.
/// Compose with darkness, assignment and meridian constraints before selection,
/// then revalidate the actual exposure plus overhead at dispatch.
pub fn altitude_windows(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    horizon: &Horizon,
    limits: AltitudeLimits,
    searched: Interval,
) -> Result<AltitudeWindows, VisibilityError> {
    find_windows(
        position,
        site,
        orientation,
        horizon,
        limits,
        searched,
        MAX_OBSERVATIONS,
    )
}

fn find_windows(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    horizon: &Horizon,
    limits: AltitudeLimits,
    searched: Interval,
    budget: usize,
) -> Result<AltitudeWindows, VisibilityError> {
    let endpoints = validate_span(position, site, orientation, searched)?;
    for point in endpoints {
        altitude_allowed(
            horizon,
            limits,
            point.azimuth_degrees,
            point.altitude_degrees,
        )?;
    }
    let mut result = AltitudeWindows {
        searched,
        windows: vec![],
        unresolved: vec![],
    };
    let mut observations = 2;
    let mut pending = vec![searched];
    while let Some(part) = pending.pop() {
        if observations >= budget {
            return Err(VisibilityError::SpanBudgetExceeded);
        }
        let (midpoint, radius) = midpoint_and_radius(part);
        let point = observe(position, site, orientation, midpoint)?;
        observations += 1;
        match classify_cap(horizon, limits, point, radius)? {
            CapClearance::Clear => append(&mut result.windows, part)?,
            CapClearance::Blocked => (),
            CapClearance::Unresolved if part.end_ms - part.start_ms <= RESOLUTION_MS => {
                append(&mut result.unresolved, part)?
            }
            CapClearance::Unresolved => {
                pending.push(Interval {
                    start_ms: midpoint,
                    end_ms: part.end_ms,
                });
                pending.push(Interval {
                    start_ms: part.start_ms,
                    end_ms: midpoint,
                });
            }
        }
    }
    Ok(result)
}

fn append(windows: &mut Vec<Interval>, next: Interval) -> Result<(), VisibilityError> {
    if let Some(last) = windows.last_mut()
        && last.end_ms == next.start_ms
    {
        last.end_ms = next.end_ms;
    } else {
        if windows.len() >= MAX_WINDOWS {
            return Err(VisibilityError::TooManyVisibilityWindows);
        }
        windows.push(next);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_exhaustion_returns_no_partial_coverage() {
        let start = 1_790_409_600_000;
        assert_eq!(
            find_windows(
                IcrsPosition {
                    ra_degrees: 83.0,
                    dec_degrees: -5.0
                },
                Site {
                    latitude_degrees: 35.0,
                    longitude_degrees: -120.0,
                    elevation_meters: 1000.0
                },
                EarthOrientation {
                    ut1_minus_utc_seconds: 0.0,
                    polar_motion_x_radians: 0.0,
                    polar_motion_y_radians: 0.0,
                    valid_from_ms: start,
                    valid_until_ms: start + 86_400_001
                },
                &Horizon::FixedMinimum {},
                AltitudeLimits {
                    rig_minimum_degrees: 20.0,
                    project_minimum_degrees: 20.0,
                    horizon_offset_degrees: 0.0,
                    rig_maximum_degrees: 89.0,
                    project_maximum_degrees: 89.0
                },
                Interval {
                    start_ms: start,
                    end_ms: start + 86_400_000
                },
                4,
            ),
            Err(VisibilityError::SpanBudgetExceeded)
        );
    }

    #[test]
    fn count_limit_never_truncates_or_bridges_gaps() {
        let mut windows = vec![];
        for n in 0..MAX_WINDOWS as u64 {
            append(
                &mut windows,
                Interval {
                    start_ms: n * 3,
                    end_ms: n * 3 + 1,
                },
            )
            .unwrap();
        }
        assert_eq!(
            append(
                &mut windows,
                Interval {
                    start_ms: 1000,
                    end_ms: 1001
                }
            ),
            Err(VisibilityError::TooManyVisibilityWindows)
        );
        assert_eq!(windows.len(), MAX_WINDOWS);
        let last = windows.last().unwrap().end_ms;
        append(
            &mut windows,
            Interval {
                start_ms: last,
                end_ms: last + 1,
            },
        )
        .unwrap();
        assert_eq!(windows.len(), MAX_WINDOWS);
    }
}
