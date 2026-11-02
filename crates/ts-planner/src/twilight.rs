// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Target Scheduler's nights: the four twilight spans from a local noon,
//! which night applies at an instant, and the twilight level then.

use crate::astro::{sun_crossings, Frame, DAY_MS, HOUR_MS};
use crate::snapshot::TwilightLevel;

/// The site's wall clock: UTC plus a fixed offset. Target Scheduler works
/// in the clock of the computer running N.I.N.A.; "noon" and "midnight"
/// below are on it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clock {
    pub utc_offset_ms: i64,
}

impl Clock {
    /// The local midnight at or before an instant.
    pub fn midnight(self, utc_ms: i64) -> i64 {
        (utc_ms + self.utc_offset_ms).div_euclid(DAY_MS) * DAY_MS - self.utc_offset_ms
    }

    /// Local noon on the date of an instant.
    pub fn noon(self, utc_ms: i64) -> i64 {
        self.midnight(utc_ms) + 12 * HOUR_MS
    }
}

/// One night's twilight spans, each from the Sun's setting below a level's
/// altitude to its rising above it, starting from noon on the date.
#[derive(Clone, Debug)]
pub(crate) struct Twilight {
    pub(crate) noon_ms: i64,
    /// −0.833°: sunset to sunrise.
    pub(crate) civil: Option<(i64, i64)>,
    /// −6°.
    pub(crate) nautical: Option<(i64, i64)>,
    /// −12°.
    pub(crate) astronomical: Option<(i64, i64)>,
    /// −18°.
    pub(crate) nighttime: Option<(i64, i64)>,
}

fn span((set, rise): (Option<i64>, Option<i64>)) -> Option<(i64, i64)> {
    match (set, rise) {
        (Some(set), Some(rise)) if set < rise => Some((set, rise)),
        _ => None,
    }
}

impl Twilight {
    pub(crate) fn new(frame: &Frame, noon_ms: i64) -> Self {
        Self {
            noon_ms,
            civil: span(sun_crossings(frame, noon_ms, -0.833)),
            nautical: span(sun_crossings(frame, noon_ms, -6.0)),
            astronomical: span(sun_crossings(frame, noon_ms, -12.0)),
            nighttime: span(sun_crossings(frame, noon_ms, -18.0)),
        }
    }

    /// The span within which a level allows imaging.
    pub(crate) fn span(&self, level: TwilightLevel) -> Option<(i64, i64)> {
        match level {
            TwilightLevel::Nighttime => self.nighttime,
            TwilightLevel::Astronomical => self.astronomical,
            TwilightLevel::Nautical => self.nautical,
            TwilightLevel::Civil => self.civil,
        }
    }

    /// The darkest level whose span holds the instant, with the start
    /// excluded and the end included; `None` in daylight.
    pub(crate) fn level(&self, t: i64) -> Option<TwilightLevel> {
        let within =
            |span: Option<(i64, i64)>| span.is_some_and(|(start, end)| start < t && t <= end);
        if within(self.nighttime) {
            Some(TwilightLevel::Nighttime)
        } else if within(self.astronomical) {
            Some(TwilightLevel::Astronomical)
        } else if within(self.nautical) {
            Some(TwilightLevel::Nautical)
        } else if within(self.civil) {
            Some(TwilightLevel::Civil)
        } else {
            None
        }
    }
}
