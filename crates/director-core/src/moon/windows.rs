use super::{MoonEvidence, MoonPolicy};
use crate::ephemeris::{moon_position, separation_degrees, sun_position};
use crate::visibility::{observe, EarthOrientation, IcrsPosition, Site, VisibilityError};
use crate::windows::{Interval, MAX_WINDOWS};

/// Conservative, bounded lunar windows over one observing day. Minute cells
/// are admitted only when the worst conditions across their whole span pass.
/// BoundGeometry intersects these with allocation, horizon and meridian limits.
pub fn moon_windows(
    policy: &MoonPolicy,
    target: IcrsPosition,
    site: Site,
    orientation: EarthOrientation,
    span: Interval,
) -> Result<Vec<Interval>, VisibilityError> {
    policy
        .validate()
        .map_err(|_| VisibilityError::InvalidAltitudeLimits)?;
    if span.start_ms >= span.end_ms || span.end_ms - span.start_ms > 86_400_000 {
        return Err(VisibilityError::InvalidSpan);
    }
    // Validate the complete time range, even when avoidance is disabled.
    observe(target, site, orientation, span.start_ms)?;
    observe(target, site, orientation, span.end_ms)?;
    if !policy.enabled {
        return Ok(vec![span]);
    }
    MoonTrack::new(site, orientation, span)?.windows(policy, target)
}

struct Cell {
    span: Interval,
    moon: IcrsPosition,
    radius: f64,
    days_from_full: f64,
    altitude_degrees: f64,
}

/// One site's evidence is shared across every recipe in the immutable grant.
pub(crate) struct MoonTrack(Vec<Cell>);

impl MoonTrack {
    pub(crate) fn new(
        site: Site,
        orientation: EarthOrientation,
        span: Interval,
    ) -> Result<Self, VisibilityError> {
        if span.start_ms >= span.end_ms || span.end_ms - span.start_ms > 86_400_000 {
            return Err(VisibilityError::InvalidSpan);
        }
        let mut cells = vec![];
        let mut start = span.start_ms;
        while start < span.end_ms {
            let end = start.saturating_add(60_000).min(span.end_ms);
            let midpoint = start + (end - start) / 2;
            let moon = moon_position(midpoint);
            let sun = sun_position(midpoint).ok_or(VisibilityError::AstronomyUnavailable)?;
            // Geocentric Moon parallax and model guard, plus an envelope faster
            // than apparent sky motion. Use worse altitude/separation throughout
            // the cell, not an instantaneous sample as capture permission.
            let radius = 1.2 + (end - start) as f64 / 2000.0 * 0.01;
            // Great-circle elongation differs from ecliptic phase near conjunction.
            // Half a day covers lunar latitude; move toward full for the bound.
            let days_from_full = ((180.0 - separation_degrees(sun, moon)) * 29.5 / 360.0
                - 0.5
                - radius * 29.5 / 360.0)
                .max(0.0);
            cells.push(Cell {
                span: Interval {
                    start_ms: start,
                    end_ms: end,
                },
                moon,
                radius,
                days_from_full,
                altitude_degrees: (observe(moon, site, orientation, midpoint)?.altitude_degrees
                    + radius)
                    .min(90.0),
            });
            start = end;
        }
        Ok(Self(cells))
    }

    pub(crate) fn windows(
        &self,
        policy: &MoonPolicy,
        target: IcrsPosition,
    ) -> Result<Vec<Interval>, VisibilityError> {
        let mut windows: Vec<Interval> = vec![];
        for cell in &self.0 {
            let evidence = MoonEvidence {
                days_from_full: cell.days_from_full,
                altitude_degrees: cell.altitude_degrees,
                separation_degrees: (separation_degrees(target, cell.moon) - cell.radius).max(0.0),
            };
            if policy
                .evaluate(evidence)
                .map_err(|_| VisibilityError::AstronomyUnavailable)?
                .allowed
            {
                if let Some(last) = windows.last_mut()
                    && last.end_ms == cell.span.start_ms
                {
                    last.end_ms = cell.span.end_ms;
                } else {
                    if windows.len() >= MAX_WINDOWS {
                        return Err(VisibilityError::TooManyVisibilityWindows);
                    }
                    windows.push(cell.span);
                }
            }
        }
        Ok(windows)
    }
}
