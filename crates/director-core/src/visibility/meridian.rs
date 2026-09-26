use super::span::{longitude_radius_degrees, midpoint_and_radius, validate_span, MAX_OBSERVATIONS};
use super::{observe_with_declination, EarthOrientation, IcrsPosition, Site, VisibilityError};
use crate::windows::{Interval, MeridianExclusion, MAX_WINDOWS};
use serde::{Deserialize, Serialize};

const RESOLUTION_MS: u64 = 1000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MeridianWindows {
    pub assignment: Interval,
    /// None only when both exclusion margins are explicitly zero.
    pub searched: Option<Interval>,
    pub windows: Vec<Interval>,
    /// Conservative bounds on possible upper-meridian crossings, not exact
    /// transit predictions or commands to flip. Bounds can include near misses.
    pub possible_transits: Vec<Interval>,
}

/// Remove possible upper-meridian crossings and independent before/after margins
/// from the assignment. Search includes crossings outside the assignment whose
/// margins could overlap it. Geometry/limits/budget failures return no windows.
/// These windows enforce exclusion only, not horizon, darkness or local flip
/// safety. Intersect them with other constraints; never use them as flip times.
/// An explicitly disabled (zero/zero) policy returns the valid assignment without
/// reading geometry. Otherwise the expanded search is limited to 24 hours.
pub fn meridian_windows(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    assignment: Interval,
    policy: MeridianExclusion,
) -> Result<MeridianWindows, VisibilityError> {
    calculate(
        position,
        site,
        orientation,
        assignment,
        policy,
        MAX_OBSERVATIONS,
    )
}

fn calculate(
    position: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    assignment: Interval,
    policy: MeridianExclusion,
    budget: usize,
) -> Result<MeridianWindows, VisibilityError> {
    if assignment.start_ms >= assignment.end_ms {
        return Err(VisibilityError::InvalidSpan);
    }
    if policy.before_ms == 0 && policy.after_ms == 0 {
        return Ok(MeridianWindows {
            assignment,
            searched: None,
            windows: vec![assignment],
            possible_transits: vec![],
        });
    }
    let searched = Interval {
        start_ms: assignment.start_ms.saturating_sub(policy.after_ms),
        end_ms: assignment
            .end_ms
            .checked_add(policy.before_ms)
            .ok_or(VisibilityError::MeridianTimeOverflow)?,
    };
    validate_span(position, site, orientation, searched)?;
    let mut observations = 2;
    let mut possible_transits: Vec<Interval> = vec![];
    let mut pending = vec![searched];
    while let Some(part) = pending.pop() {
        if observations >= budget {
            return Err(VisibilityError::SpanBudgetExceeded);
        }
        let (midpoint, radius) = midpoint_and_radius(part);
        let (observed, declination) =
            observe_with_declination(position, site, orientation, midpoint)?;
        observations += 1;
        // SOFA observed HA/Dec is a rotation of the same horizontal direction.
        // Bound its longitude sweep instead of assuming monotonic hour angle,
        // especially near a celestial pole. +/-180 is LOWER culmination.
        let hour_angle_radius = longitude_radius_degrees(declination, radius);
        if observed.hour_angle_degrees.abs() > hour_angle_radius {
            continue;
        }
        if part.end_ms - part.start_ms <= RESOLUTION_MS {
            append(&mut possible_transits, part)?;
        } else {
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
    let mut windows = vec![];
    let mut cursor = assignment.start_ms;
    for transit in &possible_transits {
        let start = transit
            .start_ms
            .saturating_sub(policy.before_ms)
            .max(assignment.start_ms);
        let end = transit
            .end_ms
            .checked_add(policy.after_ms)
            .ok_or(VisibilityError::MeridianTimeOverflow)?
            .min(assignment.end_ms);
        if end <= cursor || start >= assignment.end_ms {
            continue;
        }
        if cursor < start {
            append(
                &mut windows,
                Interval {
                    start_ms: cursor,
                    end_ms: start,
                },
            )?;
        }
        cursor = cursor.max(end);
    }
    if cursor < assignment.end_ms {
        append(
            &mut windows,
            Interval {
                start_ms: cursor,
                end_ms: assignment.end_ms,
            },
        )?;
    }
    Ok(MeridianWindows {
        assignment,
        searched: Some(searched),
        windows,
        possible_transits,
    })
}

fn append(values: &mut Vec<Interval>, next: Interval) -> Result<(), VisibilityError> {
    if let Some(last) = values.last_mut()
        && last.end_ms == next.start_ms
    {
        last.end_ms = next.end_ms;
    } else {
        if values.len() >= MAX_WINDOWS {
            return Err(VisibilityError::TooManyVisibilityWindows);
        }
        values.push(next);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_work_cannot_return_partial_transit_evidence() {
        let start = 1_790_409_600_000;
        assert_eq!(
            calculate(
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
                    valid_from_ms: start - 1000,
                    valid_until_ms: start + 100_001
                },
                Interval {
                    start_ms: start,
                    end_ms: start + 60_000
                },
                MeridianExclusion {
                    before_ms: 1000,
                    after_ms: 1000
                },
                2,
            ),
            Err(VisibilityError::SpanBudgetExceeded)
        );
    }
}
