// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A target's night as Target Scheduler samples it: altitudes from sunset
//! to just past sunrise at a fixed interval, the next visible interval of
//! at least the minimum time, the transit, and the clippers for the
//! meridian window, the maximum altitude and the meridian flip.

use crate::astro::{Frame, DAY_MS, HOUR_MS};
use crate::twilight::{Clock, Twilight};

/// Seconds, truncated, as Target Scheduler compares durations.
pub(crate) fn seconds(from: i64, to: i64) -> i64 {
    (to - from) / 1000
}

#[derive(Clone, Debug)]
pub(crate) struct Visibility {
    pub(crate) sunset: i64,
    interval: i64,
    altitudes: Vec<f64>,
    visible: Vec<bool>,
    pub(crate) imaging_possible: bool,
    pub(crate) transit: Option<i64>,
    /// The one run of samples above the maximum altitude, if any.
    exceeded: Option<(i64, i64)>,
}

impl Visibility {
    /// Sample a target over a night. `horizon` gives the altitude a target
    /// must clear at an azimuth.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        frame: &Frame,
        clock: Clock,
        twilight: &Twilight,
        interval_ms: i64,
        ra_hours: f64,
        dec_degrees: f64,
        maximum_altitude: f64,
        horizon: &dyn Fn(f64) -> f64,
    ) -> Option<Self> {
        let (sunset, sunrise) = twilight.civil?;
        let length = seconds(sunset, sunrise);
        let mut altitudes = Vec::new();
        let mut visible = Vec::new();
        if length >= 300 {
            let count = length / (interval_ms / 1000) + 2;
            for i in 0..count {
                let t = sunset + i * interval_ms;
                let (altitude, azimuth) = frame.horizontal(ra_hours, dec_degrees, t);
                altitudes.push(altitude);
                visible.push(altitude - horizon(azimuth) > 0.0);
            }
        }
        let imaging_possible = altitudes.iter().any(|a| *a > 0.0);
        let mut found = Self {
            sunset,
            interval: interval_ms,
            altitudes,
            visible,
            imaging_possible,
            transit: None,
            exceeded: None,
        };
        if imaging_possible {
            let midnight = clock.midnight(sunset);
            let earliest = sunset - 2 * HOUR_MS;
            let latest = sunrise + 2 * HOUR_MS;
            let first = frame.transit_after(midnight, ra_hours);
            found.transit = if first > earliest && first < latest {
                Some(first)
            } else if first < sunset {
                let second = frame.transit_after(midnight + DAY_MS, ra_hours);
                (second > earliest && second < latest).then_some(second)
            } else {
                None
            };
        }
        let latitude = frame.latitude_degrees();
        if maximum_altitude != 0.0 && 90.0 - (latitude - dec_degrees).abs() > maximum_altitude {
            let from = found.find_interval(sunset);
            let to = found.find_interval(sunrise);
            if let Some(start) = (from..=to).find(|&i| found.altitudes[i] > maximum_altitude) {
                let end = (start..=to)
                    .find(|&i| found.altitudes[i] <= maximum_altitude)
                    .unwrap_or(to);
                found.exceeded = Some((found.time(start), found.time(end)));
            }
        }
        Some(found)
    }

    fn time(&self, index: usize) -> i64 {
        self.sunset + index as i64 * self.interval
    }

    fn len(&self) -> usize {
        self.altitudes.len()
    }

    /// The last sample at or before `t`: the first before them all, the
    /// last after.
    fn find_interval(&self, t: i64) -> usize {
        if self.len() == 0 || t < self.time(0) {
            return 0;
        }
        let index = ((t - self.sunset) / self.interval) as usize;
        index.min(self.len() - 1)
    }

    /// Whether the target is above its horizon at the sample at or before `t`.
    pub(crate) fn visible_at(&self, t: i64) -> bool {
        self.len() > 0 && t >= self.sunset && self.visible[self.find_interval(t)]
    }

    /// The next visible interval from `t` inside `span`.
    fn next_visible(&self, t: i64, span: (i64, i64)) -> Option<(i64, i64)> {
        if !self.imaging_possible {
            return None;
        }
        if t < span.0 {
            return self.next_visible(span.0, span);
        }
        if t >= span.1 {
            return None;
        }
        for i in self.find_interval(t)..self.len() {
            if self.time(i) >= span.1 {
                break;
            }
            if self.visible[i] {
                let mut j = i + 1;
                let stop = loop {
                    if j == self.len() {
                        break span.1;
                    }
                    if !self.visible[j] {
                        break self.time(j);
                    }
                    if self.time(j) >= span.1 {
                        break span.1;
                    }
                    j += 1;
                };
                return Some((self.time(i), stop));
            }
        }
        None
    }

    /// The next visible interval from `t` at least `minimum_seconds` long.
    pub(crate) fn next_visible_for(
        &self,
        t: i64,
        span: (i64, i64),
        minimum_seconds: i64,
    ) -> Option<(i64, i64)> {
        let mut found = self.next_visible(t, span);
        while let Some((start, stop)) = found {
            if seconds(start, stop) >= minimum_seconds {
                return Some((start, stop));
            }
            found = self.next_visible(stop, span);
        }
        None
    }

    /// Clip `[start, end]` to keep out of the run above the maximum altitude.
    pub(crate) fn clip_maximum(&self, start: i64, end: i64) -> Option<(i64, i64)> {
        let Some((xs, xe)) = self.exceeded else {
            return Some((start, end));
        };
        let inside = |t: i64| xs <= t && t <= xe;
        if end < xs {
            Some((start, end))
        } else if start < xs && inside(end) {
            Some((start, xs))
        } else if inside(start) && inside(end) {
            None
        } else if inside(start) && end > xe {
            Some((xe, end))
        } else if start > xe {
            Some((start, end))
        } else {
            // Starts before the run and ends after it: the part after is
            // found by a later call.
            Some((start, xs))
        }
    }

    pub(crate) fn next_safe_start(&self, start: i64, end: i64) -> i64 {
        match self.exceeded {
            None => start,
            Some((_, xe)) => self.clip_maximum(start, end).map_or(xe, |(s, _)| s),
        }
    }
}

/// Clip to the meridian window: `minutes` either side of the transit.
pub(crate) fn clip_meridian_window(
    start: i64,
    transit: Option<i64>,
    end: i64,
    window_ms: i64,
) -> Option<(i64, i64)> {
    let transit = transit?;
    if start > transit + window_ms || end < transit - window_ms {
        return None;
    }
    let window_seconds = window_ms / 1000;
    let start = if seconds(start, transit) > window_seconds {
        transit - window_ms
    } else {
        start
    };
    let end = if seconds(transit, end) > window_seconds {
        transit + window_ms
    } else {
        end
    };
    Some((start, end))
}

/// The no-flip zone around the transit, padded by 30 seconds each side.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FlipZone {
    pub(crate) pause_before_ms: i64,
    pub(crate) after_ms: i64,
}

const FLIP_PADDING_MS: i64 = 30_000;

impl FlipZone {
    /// When imaging may resume after the zone.
    pub(crate) fn safe_after(self, transit: i64) -> i64 {
        transit + self.after_ms + FLIP_PADDING_MS
    }

    pub(crate) fn clip(self, start: i64, transit: Option<i64>, end: i64) -> Option<(i64, i64)> {
        let Some(transit) = transit else {
            return Some((start, end));
        };
        let pause = transit - self.pause_before_ms - FLIP_PADDING_MS;
        let after = self.safe_after(transit);
        // Starts before the pause and runs into the zone or past it.
        if start < pause && (end > after || (pause < end && end < after)) {
            Some((start, pause))
        } else if pause < start && start < after && after < end {
            Some((after, end))
        } else if pause < start && start < after && pause < end && end < after {
            None
        } else {
            Some((start, end))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_meridian_window_and_flip_zone_clip_as_target_scheduler_does() {
        let hour = HOUR_MS;
        let transit = Some(10 * hour);
        assert_eq!(
            clip_meridian_window(6 * hour, transit, 14 * hour, hour),
            Some((9 * hour, 11 * hour))
        );
        assert_eq!(
            clip_meridian_window(12 * hour, transit, 14 * hour, hour),
            None
        );
        assert_eq!(clip_meridian_window(6 * hour, None, 14 * hour, hour), None);
        let zone = FlipZone {
            pause_before_ms: 10 * 60_000,
            after_ms: 5 * 60_000,
        };
        let pause = 10 * hour - 10 * 60_000 - 30_000;
        let after = 10 * hour + 5 * 60_000 + 30_000;
        assert_eq!(
            zone.clip(6 * hour, transit, 14 * hour),
            Some((6 * hour, pause))
        );
        assert_eq!(
            zone.clip(10 * hour, transit, 14 * hour),
            Some((after, 14 * hour))
        );
        assert_eq!(zone.clip(10 * hour, transit, 10 * hour + 60_000), None);
        // A window ending exactly at the zone's end is left whole.
        assert_eq!(
            zone.clip(10 * hour, transit, after),
            Some((10 * hour, after))
        );
    }
}
