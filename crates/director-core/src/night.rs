//! Night-by-night feasibility: for a site and its horizon, how long each
//! coming night is dark, how long each target sits inside the rig's limits
//! during that darkness, and what the Moon is doing. A planning estimate
//! from sampled positions; the rig still decides at dispatch time.

use crate::ephemeris::{moon_illumination, moon_position, separation_degrees, sun_position};
use crate::visibility::{
    altitude_allowed, observe, AltitudeLimits, EarthOrientation, Horizon, IcrsPosition, Site,
    VisibilityError,
};
use serde::{Deserialize, Serialize};

pub const MAX_NIGHTS: u32 = 31;
pub const MAX_TARGETS: usize = 16;
pub const MIN_STEP_MS: u64 = 60_000;
const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NightTarget {
    pub id: String,
    pub position: IcrsPosition,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NightRequest {
    pub site: Site,
    pub horizon: Horizon,
    pub limits: AltitudeLimits,
    pub targets: Vec<NightTarget>,
    /// The first night is the one whose local noon comes at or after this.
    pub start_ms: u64,
    pub nights: u32,
    pub step_ms: u64,
    /// Sun altitude below which it counts as dark, such as -12 or -18.
    pub dark_below_degrees: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetNight {
    pub id: String,
    /// Dark hours with the target above the horizon curve and inside limits.
    pub hours_up: f64,
    /// The same, while the Moon is below the horizon.
    pub hours_up_moon_down: f64,
    pub max_altitude_degrees: f64,
    pub min_moon_separation_degrees: f64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Night {
    /// Local calendar date of the evening, `YYYY-MM-DD`.
    pub date: String,
    pub noon_ms: u64,
    pub dusk_ms: Option<u64>,
    pub dawn_ms: Option<u64>,
    pub dark_hours: f64,
    pub moon_illumination: f64,
    pub moon_hours_up_in_dark: f64,
    pub targets: Vec<TargetNight>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NightError {
    InvalidRequest,
    Visibility(VisibilityError),
}

impl From<VisibilityError> for NightError {
    fn from(error: VisibilityError) -> Self {
        Self::Visibility(error)
    }
}

/// Planning-grade Earth orientation: zero corrections, valid for the search.
/// Good to about an arcsecond of pointing, which twilight and Moon timing do
/// not feel; the rig's own geometry uses real orientation data.
fn planning_orientation(start_ms: u64, end_ms: u64) -> EarthOrientation {
    EarthOrientation {
        ut1_minus_utc_seconds: 0.0,
        polar_motion_x_radians: 0.0,
        polar_motion_y_radians: 0.0,
        valid_from_ms: start_ms.saturating_sub(DAY_MS),
        valid_until_ms: end_ms + DAY_MS,
    }
}

/// The UTC instant of the first local mean noon at or after `start_ms`.
fn first_local_noon(start_ms: u64, longitude_degrees: f64) -> u64 {
    let offset_ms = (longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
    let day_start = (start_ms / DAY_MS) * DAY_MS;
    let mut noon = day_start as i64 + (DAY_MS / 2) as i64 - offset_ms;
    while noon < start_ms as i64 {
        noon += DAY_MS as i64;
    }
    while noon - DAY_MS as i64 >= start_ms as i64 {
        noon -= DAY_MS as i64;
    }
    noon as u64
}

fn local_date(noon_ms: u64, longitude_degrees: f64) -> String {
    let offset_ms = (longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
    let local = noon_ms as i64 + offset_ms;
    chrono::DateTime::from_timestamp_millis(local)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

pub fn night_preview(request: &NightRequest) -> Result<Vec<Night>, NightError> {
    request.site.validate()?;
    request.horizon.validate()?;
    request.limits.validate()?;
    if request.nights == 0
        || request.nights > MAX_NIGHTS
        || request.step_ms < MIN_STEP_MS
        || request.step_ms > DAY_MS / 24
        || request.targets.len() > MAX_TARGETS
        || !request.dark_below_degrees.is_finite()
        || !(-30.0..=0.0).contains(&request.dark_below_degrees)
    {
        return Err(NightError::InvalidRequest);
    }
    let first_noon = first_local_noon(request.start_ms, request.site.longitude_degrees);
    let end = first_noon + u64::from(request.nights) * DAY_MS;
    let orientation = planning_orientation(first_noon, end);
    let step_hours = request.step_ms as f64 / 3_600_000.0;
    let mut nights = Vec::with_capacity(request.nights as usize);
    for index in 0..request.nights {
        let noon = first_noon + u64::from(index) * DAY_MS;
        let next_noon = noon + DAY_MS;
        let mut dusk = None;
        let mut dawn = None;
        let mut dark_hours = 0.0;
        let mut moon_hours = 0.0;
        let mut per_target: Vec<TargetNight> = request
            .targets
            .iter()
            .map(|target| TargetNight {
                id: target.id.clone(),
                hours_up: 0.0,
                hours_up_moon_down: 0.0,
                max_altitude_degrees: -90.0,
                min_moon_separation_degrees: 180.0,
            })
            .collect();
        let mut t = noon;
        while t < next_noon {
            let sun = sun_position(t).ok_or(VisibilityError::AstronomyUnavailable)?;
            let sun_altitude = observe(sun, request.site, orientation, t)?.altitude_degrees;
            let dark = sun_altitude < request.dark_below_degrees;
            if dark {
                if dusk.is_none() {
                    dusk = Some(t);
                }
                dawn = Some(t + request.step_ms);
                dark_hours += step_hours;
                let moon = moon_position(t);
                let moon_up = observe(moon, request.site, orientation, t)?.altitude_degrees > 0.0;
                if moon_up {
                    moon_hours += step_hours;
                }
                for (target, entry) in request.targets.iter().zip(per_target.iter_mut()) {
                    let observed = observe(target.position, request.site, orientation, t)?;
                    entry.max_altitude_degrees =
                        entry.max_altitude_degrees.max(observed.altitude_degrees);
                    entry.min_moon_separation_degrees = entry
                        .min_moon_separation_degrees
                        .min(separation_degrees(target.position, moon));
                    if altitude_allowed(
                        &request.horizon,
                        request.limits,
                        observed.azimuth_degrees,
                        observed.altitude_degrees,
                    )? {
                        entry.hours_up += step_hours;
                        if !moon_up {
                            entry.hours_up_moon_down += step_hours;
                        }
                    }
                }
            }
            t += request.step_ms;
        }
        let midnight = noon + DAY_MS / 2;
        nights.push(Night {
            date: local_date(noon, request.site.longitude_degrees),
            noon_ms: noon,
            dusk_ms: dusk,
            dawn_ms: dawn,
            dark_hours,
            moon_illumination: moon_illumination(midnight).unwrap_or(0.0),
            moon_hours_up_in_dark: moon_hours,
            targets: per_target,
        });
    }
    Ok(nights)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> NightRequest {
        NightRequest {
            site: Site {
                latitude_degrees: 34.2,
                longitude_degrees: -118.3,
                elevation_meters: 400.0,
            },
            horizon: Horizon::FixedMinimum {},
            limits: AltitudeLimits {
                rig_minimum_degrees: 25.0,
                project_minimum_degrees: -90.0,
                horizon_offset_degrees: 0.0,
                rig_maximum_degrees: 90.0,
                project_maximum_degrees: 90.0,
            },
            targets: vec![
                NightTarget {
                    id: "heart".into(),
                    position: IcrsPosition {
                        ra_degrees: 38.2,
                        dec_degrees: 61.45,
                    },
                },
                NightTarget {
                    id: "south".into(),
                    position: IcrsPosition {
                        ra_degrees: 38.2,
                        dec_degrees: -75.0,
                    },
                },
            ],
            // 2026-09-25 00:00 UTC
            start_ms: 1_790_294_400_000,
            nights: 2,
            step_ms: 300_000,
            dark_below_degrees: -12.0,
        }
    }

    #[test]
    fn a_los_angeles_september_night_is_about_ten_dark_hours_with_a_full_moon() {
        let nights = night_preview(&request()).unwrap();
        assert_eq!(nights.len(), 2);
        let first = &nights[0];
        assert_eq!(first.date, "2026-09-25");
        assert!(
            first.dark_hours > 9.0 && first.dark_hours < 11.0,
            "{first:?}"
        );
        let dusk = first.dusk_ms.unwrap();
        let dawn = first.dawn_ms.unwrap();
        // Local mean noon is 19:53 UTC; nautical dusk falls near 02:40 UTC
        // and nautical dawn near 12:55 UTC.
        assert!(
            dusk > first.noon_ms + 6 * 3_600_000 && dusk < first.noon_ms + 8 * 3_600_000,
            "{dusk}"
        );
        assert!(
            dawn > first.noon_ms + 16 * 3_600_000 && dawn < first.noon_ms + 19 * 3_600_000,
            "{dawn}"
        );
        assert!(
            first.moon_illumination > 0.95,
            "{}",
            first.moon_illumination
        );
        assert!(first.moon_hours_up_in_dark > 5.0);
        let heart = &first.targets[0];
        // The Heart Nebula rides high all night from 34 N in September.
        assert!(heart.hours_up > 7.0, "{heart:?}");
        assert!(heart.max_altitude_degrees > 55.0);
        assert!(heart.hours_up_moon_down < heart.hours_up);
        assert!(heart.min_moon_separation_degrees > 20.0);
        // A far-southern target never clears 25 degrees from there.
        assert_eq!(first.targets[1].hours_up, 0.0);
        assert!(first.targets[1].max_altitude_degrees < 0.0);
        assert_eq!(nights[1].date, "2026-09-26");
    }

    #[test]
    fn a_custom_horizon_and_a_polar_summer_are_honoured() {
        let mut walled = request();
        walled.horizon = Horizon::Custom {
            points: vec![
                crate::visibility::HorizonPoint {
                    azimuth_degrees: 0.0,
                    altitude_degrees: 80.0,
                },
                crate::visibility::HorizonPoint {
                    azimuth_degrees: 360.0,
                    altitude_degrees: 80.0,
                },
            ],
        };
        let nights = night_preview(&walled).unwrap();
        assert_eq!(nights[0].targets[0].hours_up, 0.0);
        assert!(nights[0].dark_hours > 9.0);
        let mut midsummer = request();
        midsummer.site.latitude_degrees = 69.6;
        midsummer.site.longitude_degrees = 18.9;
        midsummer.start_ms = 1_781_740_800_000; // 2026-06-18
        midsummer.nights = 1;
        let nights = night_preview(&midsummer).unwrap();
        assert_eq!(nights[0].dark_hours, 0.0);
        assert_eq!(nights[0].dusk_ms, None);
        let mut bad = request();
        bad.nights = 0;
        assert_eq!(night_preview(&bad), Err(NightError::InvalidRequest));
        bad = request();
        bad.step_ms = 1_000;
        assert_eq!(night_preview(&bad), Err(NightError::InvalidRequest));
    }
}
