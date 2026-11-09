//! Night-by-night feasibility: for a site and its horizon, how long each
//! coming night is dark, how long each target sits inside the rig's limits
//! during that darkness, and what the Moon is doing. A planning estimate
//! from sampled positions; the rig still decides at dispatch time.

use crate::ephemeris::{moon_illumination, moon_position, separation_degrees, sun_position};
use crate::moon::{MoonCell, MoonPolicy, MoonTrack};
use crate::visibility::{
    altitude_allowed, meridian_windows, AltitudeLimits, EarthOrientation, Horizon, IcrsPosition,
    ObservedPosition, Observer, Site, VisibilityError,
};
use crate::windows::{Interval, MeridianExclusion};
use serde::{Deserialize, Serialize};

pub const MAX_NIGHTS: u32 = 31;
pub const MAX_TARGETS: usize = 16;
pub const MIN_STEP_MS: u64 = 60_000;
/// Most availability windows one request may carry: a few a night.
pub const MAX_AVAILABLE_WINDOWS: usize = 256;
/// Most Moon rules one target may carry: one per filter.
pub const MAX_MOON_POLICIES: usize = 16;
const DAY_MS: u64 = 86_400_000;
/// The span `start_ms` may fall in: a day after the Unix epoch, so the noon
/// before it is never earlier, to 2099-01-01, so the last night still ends
/// inside the years the astrometry covers.
pub const EARLIEST_START_MS: u64 = DAY_MS;
pub const LATEST_START_MS: u64 = 4_070_908_800_000;
/// Longest meridian window, either side: half a day covers the whole sky.
const MAX_MERIDIAN_WINDOW_MS: u64 = DAY_MS / 2;
/// How far the hour angle turns in an hour of clock time, in degrees.
const SIDEREAL_DEGREES_PER_HOUR: f64 = 15.041_068_64;

/// A night's key: the local date of its evening, `YYYY-MM-DD`, as
/// [`Night::date`] gives it. Availability marks and overrides use it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct NightDate(chrono::NaiveDate);

/// Text that is not a date from 1970-01-02 to 2098-12-31 as `YYYY-MM-DD`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidNightDate;

impl std::fmt::Display for InvalidNightDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a night is a date from 1970-01-02 to 2098-12-31, written YYYY-MM-DD")
    }
}

impl std::error::Error for InvalidNightDate {}

impl std::str::FromStr for NightDate {
    type Err = InvalidNightDate;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let date =
            chrono::NaiveDate::parse_from_str(text, "%Y-%m-%d").map_err(|_| InvalidNightDate)?;
        let earliest = chrono::NaiveDate::from_ymd_opt(1970, 1, 2).ok_or(InvalidNightDate)?;
        let latest = chrono::NaiveDate::from_ymd_opt(2098, 12, 31).ok_or(InvalidNightDate)?;
        // Only the canonical spelling: `2026-1-5` would key a second mark.
        if !(earliest..=latest).contains(&date) || date.format("%Y-%m-%d").to_string() != text {
            return Err(InvalidNightDate);
        }
        Ok(Self(date))
    }
}

impl std::fmt::Display for NightDate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0.format("%Y-%m-%d"))
    }
}

impl TryFrom<String> for NightDate {
    type Error = InvalidNightDate;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<NightDate> for String {
    fn from(date: NightDate) -> Self {
        date.to_string()
    }
}

impl NightDate {
    /// The night under way at an instant at this longitude, as
    /// [`NightRequest::start_ms`] picks the first night.
    pub fn at(instant_ms: u64, longitude_degrees: f64) -> Option<Self> {
        let noon = night_noon(instant_ms, longitude_degrees)?;
        local_date(noon, longitude_degrees).parse().ok()
    }

    /// The night's span at this longitude, local mean noon to the next: the
    /// span [`night_preview`] reports for it.
    pub fn span(self, longitude_degrees: f64) -> Interval {
        let offset_ms = (longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
        let midnight = self
            .0
            .and_hms_opt(0, 0, 0)
            .map_or(0, |t| t.and_utc().timestamp_millis());
        let noon = (midnight + DAY_MS as i64 / 2 - offset_ms).max(0) as u64;
        Interval {
            start_ms: noon,
            end_ms: noon + DAY_MS,
        }
    }

    /// Nights from this one to `other`: 0 for the same night, negative when
    /// `other` comes first.
    pub fn days_until(self, other: Self) -> i64 {
        (other.0 - self.0).num_days()
    }

    /// The night after this one, if it is still a night this type holds.
    pub fn next(self) -> Option<Self> {
        let next = self.0.succ_opt()?;
        next.format("%Y-%m-%d").to_string().parse().ok()
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NightTarget {
    pub id: String,
    pub position: IcrsPosition,
    /// The Moon rules of the filters still owed on this target. When there
    /// are any, the night counts the hours at least one of them allows
    /// (`hours_up_moon_allowed`).
    #[serde(default)]
    pub moon: Vec<MoonPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NightRequest {
    pub site: Site,
    pub horizon: Horizon,
    pub limits: AltitudeLimits,
    pub targets: Vec<NightTarget>,
    /// The first night is the one under way at this instant: it runs from
    /// the local mean noon at or before it to the next, so in the afternoon
    /// it is the coming night and after midnight the one that began the
    /// evening before.
    pub start_ms: u64,
    pub nights: u32,
    pub step_ms: u64,
    /// Sun altitude below which it counts as dark, such as -12 or -18.
    pub dark_below_degrees: f64,
    /// The rig's pause around the meridian; zero and zero means none.
    #[serde(default = "no_exclusion")]
    pub meridian_exclusion: MeridianExclusion,
    /// The project's meridian window, Target Scheduler's `meridianwindow`:
    /// image only this long either side of the upper transit. Zero is off.
    #[serde(default)]
    pub meridian_window_ms: u64,
    /// When the rig may plan to image, if not all the time: the night
    /// availability marks (or a forecast) as instants. `None` is always,
    /// so a caller that knows nothing of availability changes nothing.
    /// Planning input only; it never allows or blocks acquisition.
    #[serde(default)]
    pub available: Option<Vec<Interval>>,
}

fn no_exclusion() -> MeridianExclusion {
    MeridianExclusion {
        before_ms: 0,
        after_ms: 0,
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetNight {
    pub id: String,
    /// Dark hours with the target above the horizon curve, inside limits and
    /// the meridian window, and outside the rig's meridian pause.
    pub hours_up: f64,
    /// The same, while the Moon is below the horizon.
    pub hours_up_moon_down: f64,
    /// `hours_up` inside the request's available windows.
    #[serde(default)]
    pub hours_up_available: f64,
    /// `hours_up_available` while at least one of the target's Moon rules
    /// allows imaging; the same as `hours_up_available` without rules.
    #[serde(default)]
    pub hours_up_moon_allowed: f64,
    /// Dark hours the target was up but the meridian pause forbade.
    pub hours_lost_to_meridian: f64,
    /// The upper transit within this noon-to-noon night, when it happens.
    #[serde(deserialize_with = "Option::deserialize")]
    pub transit_ms: Option<u64>,
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
    /// `dark_hours` inside the request's available windows.
    #[serde(default)]
    pub available_dark_hours: f64,
    pub moon_illumination: f64,
    pub moon_hours_up_in_dark: f64,
    pub targets: Vec<TargetNight>,
}

/// One instant of one night, for drawing.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TargetSample {
    pub altitude_degrees: f64,
    pub azimuth_degrees: f64,
    /// The custom horizon at this azimuth plus the project's horizon
    /// offset, when the rig has one.
    #[serde(deserialize_with = "Option::deserialize")]
    pub horizon_altitude_degrees: Option<f64>,
    /// Inside the limits, above the horizon and inside the meridian window
    /// at this instant.
    pub allowed: bool,
    /// Inside the rig's meridian pause at this instant.
    pub meridian_blocked: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub t_ms: u64,
    pub sun_altitude_degrees: f64,
    pub moon_altitude_degrees: f64,
    pub targets: Vec<TargetSample>,
}

/// A night's summary with the curves behind it, noon to noon.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NightCurve {
    pub night: Night,
    pub samples: Vec<Sample>,
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
pub fn planning_orientation(start_ms: u64, end_ms: u64) -> EarthOrientation {
    EarthOrientation {
        ut1_minus_utc_seconds: 0.0,
        polar_motion_x_radians: 0.0,
        polar_motion_y_radians: 0.0,
        valid_from_ms: start_ms.saturating_sub(DAY_MS),
        valid_until_ms: end_ms + DAY_MS,
    }
}

/// The UTC instant of the last local mean noon at or before `start_ms`,
/// which opens the night under way then. None outside the Unix era.
fn night_noon(start_ms: u64, longitude_degrees: f64) -> Option<u64> {
    let offset_ms = (longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
    let day = DAY_MS as i64;
    let local = i64::try_from(start_ms).ok()?.checked_add(offset_ms)?;
    let local_noon = (local - day / 2).div_euclid(day) * day + day / 2;
    u64::try_from(local_noon - offset_ms).ok()
}

fn local_date(noon_ms: u64, longitude_degrees: f64) -> String {
    let offset_ms = (longitude_degrees / 15.0 * 3_600_000.0).round() as i64;
    let local = noon_ms as i64 + offset_ms;
    chrono::DateTime::from_timestamp_millis(local)
        .map(|t| t.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

fn validate(request: &NightRequest) -> Result<(), NightError> {
    request.site.validate()?;
    request.horizon.validate()?;
    request.limits.validate()?;
    if request.nights == 0
        || request.nights > MAX_NIGHTS
        || !(EARLIEST_START_MS..=LATEST_START_MS).contains(&request.start_ms)
        || request.meridian_window_ms > MAX_MERIDIAN_WINDOW_MS
        || request.step_ms < MIN_STEP_MS
        || request.step_ms > DAY_MS / 24
        || request.targets.len() > MAX_TARGETS
        || request.targets.iter().any(|target| {
            target.moon.len() > MAX_MOON_POLICIES
                || target.moon.iter().any(|policy| policy.validate().is_err())
        })
        || !request.dark_below_degrees.is_finite()
        || !(-30.0..=0.0).contains(&request.dark_below_degrees)
        || request.available.as_ref().is_some_and(|windows| {
            windows.len() > MAX_AVAILABLE_WINDOWS || windows.iter().any(|w| w.start_ms >= w.end_ms)
        })
    {
        return Err(NightError::InvalidRequest);
    }
    Ok(())
}

/// The share of `[start, start + step)` inside the windows, 0 to 1; 1 with
/// no windows. Windows may overlap, so they are merged first.
fn available_share(windows: Option<&[Interval]>, start_ms: u64, step_ms: u64) -> f64 {
    let Some(windows) = windows else {
        return 1.0;
    };
    let end_ms = start_ms + step_ms;
    let mut covered = 0;
    let mut reached = start_ms;
    for window in windows {
        let from = window.start_ms.max(reached);
        let to = window.end_ms.min(end_ms);
        if to > from {
            covered += to - from;
            reached = to;
        }
    }
    covered as f64 / step_ms as f64
}

/// The instants in both sorted, merged lists.
fn intersection(a: &[Interval], b: &[Interval]) -> Vec<Interval> {
    let mut result = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let start = a[i].start_ms.max(b[j].start_ms);
        let end = a[i].end_ms.min(b[j].end_ms);
        if start < end {
            result.push(Interval {
                start_ms: start,
                end_ms: end,
            });
        }
        if a[i].end_ms <= b[j].end_ms {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

fn merged(windows: &[Interval]) -> Vec<Interval> {
    let mut sorted = windows.to_vec();
    sorted.sort_by_key(|w| (w.start_ms, w.end_ms));
    let mut result: Vec<Interval> = Vec::with_capacity(sorted.len());
    for window in sorted {
        match result.last_mut() {
            Some(previous) if window.start_ms <= previous.end_ms => {
                previous.end_ms = previous.end_ms.max(window.end_ms);
            }
            _ => result.push(window),
        }
    }
    result
}

pub fn night_preview(request: &NightRequest) -> Result<Vec<Night>, NightError> {
    validate(request)?;
    (0..request.nights)
        .map(|index| one_night(request, index, false, None).map(|curve| curve.night))
        .collect()
}

/// One night's summary, by index from the first: what [`night_preview`]
/// gives for that night alone, so a caller can spread the nights over
/// threads.
pub fn night_summary(request: &NightRequest, index: u32) -> Result<Night, NightError> {
    validate(request)?;
    if index >= request.nights {
        return Err(NightError::InvalidRequest);
    }
    one_night(request, index, false, None).map(|curve| curve.night)
}

/// The curves for one of the requested nights, by index from the first.
pub fn night_curve(request: &NightRequest, index: u32) -> Result<NightCurve, NightError> {
    validate(request)?;
    if index >= request.nights {
        return Err(NightError::InvalidRequest);
    }
    one_night(request, index, true, None)
}

/// [`night_summary`] under a sky computed beforehand: the same night, to
/// the bit, without computing the Sun, the Moon and the Earth again. The
/// sky must be the night's, at the request's site and step:
/// [`NightRequest::noon_ms`] gives its noon.
pub fn night_summary_under(
    request: &NightRequest,
    index: u32,
    sky: &NightSky,
) -> Result<Night, NightError> {
    validate(request)?;
    if index >= request.nights {
        return Err(NightError::InvalidRequest);
    }
    one_night(request, index, false, Some(sky)).map(|curve| curve.night)
}

/// [`night_curve`] under a sky computed beforehand, as
/// [`night_summary_under`] takes it.
pub fn night_curve_under(
    request: &NightRequest,
    index: u32,
    sky: &NightSky,
) -> Result<NightCurve, NightError> {
    validate(request)?;
    if index >= request.nights {
        return Err(NightError::InvalidRequest);
    }
    one_night(request, index, true, Some(sky))
}

impl NightRequest {
    /// The local mean noon that opens the night `index` from the first: the
    /// noon of the [`NightSky`] its summary takes.
    pub fn noon_ms(&self, index: u32) -> Option<u64> {
        let first = night_noon(self.start_ms, self.site.longitude_degrees)?;
        first.checked_add(u64::from(index).checked_mul(DAY_MS)?)
    }
}

/// The Earth orientation a night's sky is computed with: planning grade,
/// valid for the night.
fn sky_orientation(noon_ms: u64) -> EarthOrientation {
    planning_orientation(noon_ms, noon_ms + DAY_MS)
}

/// The Sun, the Moon and the Earth at one instant of a night, as every
/// target's sums that night take them.
#[derive(Clone, Copy)]
pub struct SkyStep {
    t_ms: u64,
    /// The star-independent part of each observation at the instant.
    observer: Observer,
    sun_altitude_degrees: f64,
    moon: IcrsPosition,
}

impl SkyStep {
    /// The sky at `t_ms`, one of [`NightSky::instants`] of the night that
    /// opens at `noon_ms`.
    pub fn at(site: Site, noon_ms: u64, t_ms: u64) -> Result<Self, NightError> {
        let observer = Observer::at(site, sky_orientation(noon_ms), t_ms)?;
        let sun = sun_position(t_ms).ok_or(VisibilityError::AstronomyUnavailable)?;
        let sun_altitude_degrees = observer.observe(sun)?.altitude_degrees;
        Ok(Self {
            t_ms,
            observer,
            sun_altitude_degrees,
            moon: moon_position(t_ms),
        })
    }
}

/// One minute of a night's Moon track, for the Moon rules.
#[derive(Clone, Copy)]
pub struct SkyMoonCell(MoonCell);

impl SkyMoonCell {
    /// The cell over `span`, one of [`NightSky::moon_spans`] of the night
    /// that opens at `noon_ms`.
    pub fn at(site: Site, noon_ms: u64, span: Interval) -> Result<Self, NightError> {
        Ok(Self(MoonCell::at(site, sky_orientation(noon_ms), span)?))
    }
}

/// What every target's hours in one night at one site share: the Sun's
/// altitude, the Moon and the Earth's orientation at each step, and the
/// Moon track its Moon rules are judged on. Computing it is most of a
/// night's work, so a caller planning many targets at one site, or the
/// same night again, computes it once and passes it to
/// [`night_summary_under`]. The summaries are the same to the bit.
///
/// [`NightSky::new`] computes it on the calling thread. A caller with
/// threads to spare computes each of [`NightSky::instants`] with
/// [`SkyStep::at`] and each of [`NightSky::moon_spans`] with
/// [`SkyMoonCell::at`], in any order, and puts them together with
/// [`NightSky::from_steps`] and [`NightSky::set_moon_track`].
pub struct NightSky {
    site: Site,
    noon_ms: u64,
    step_ms: u64,
    steps: Vec<SkyStep>,
    /// Built on first use, by the first summary whose targets have Moon
    /// rules, unless set beforehand.
    track: std::sync::OnceLock<Result<MoonTrack, VisibilityError>>,
    /// Each target's windows outside a meridian pause, by its position, the
    /// pause and the span searched: kept as summaries ask for them.
    meridian: std::sync::Mutex<std::collections::HashMap<MeridianKey, MeridianFound>>,
    /// The Moon's altitude every quarter hour, kept once asked for.
    moon_strip: std::sync::OnceLock<Vec<Option<f64>>>,
}

/// A target's meridian search, to the bit: right ascension, declination,
/// the pause before and after, and the span searched.
type MeridianKey = (u64, u64, u64, u64, u64, u64);
type MeridianFound = Result<Vec<Interval>, VisibilityError>;
/// The most meridian searches one sky keeps; past it they are computed
/// each time.
const MAX_MERIDIAN_KEPT: usize = 4096;
/// The Moon strip's step.
pub const MOON_STRIP_STEP_MS: u64 = 15 * 60_000;

impl NightSky {
    /// The night that opens at `noon_ms` at `site`, sampled every
    /// `step_ms`.
    pub fn new(site: Site, noon_ms: u64, step_ms: u64) -> Result<Self, NightError> {
        let steps = Self::instants(noon_ms, step_ms)?
            .into_iter()
            .map(|t| SkyStep::at(site, noon_ms, t))
            .collect::<Result<_, _>>()?;
        Self::from_steps(site, noon_ms, step_ms, steps)
    }

    /// The instants a night's summary samples, in order.
    pub fn instants(noon_ms: u64, step_ms: u64) -> Result<Vec<u64>, NightError> {
        if !(MIN_STEP_MS..=DAY_MS / 24).contains(&step_ms) {
            return Err(NightError::InvalidRequest);
        }
        let next_noon = noon_ms
            .checked_add(DAY_MS)
            .ok_or(NightError::InvalidRequest)?;
        let mut instants = Vec::with_capacity((DAY_MS / step_ms) as usize + 1);
        let mut t = noon_ms;
        while t < next_noon {
            instants.push(t);
            t += step_ms;
        }
        Ok(instants)
    }

    /// The sky from its steps, computed apart: one per instant of
    /// [`NightSky::instants`], in order.
    pub fn from_steps(
        site: Site,
        noon_ms: u64,
        step_ms: u64,
        steps: Vec<SkyStep>,
    ) -> Result<Self, NightError> {
        let instants = Self::instants(noon_ms, step_ms)?;
        if instants.len() != steps.len()
            || instants.iter().zip(&steps).any(|(t, step)| *t != step.t_ms)
        {
            return Err(NightError::InvalidRequest);
        }
        Ok(Self {
            site,
            noon_ms,
            step_ms,
            steps,
            track: std::sync::OnceLock::new(),
            meridian: std::sync::Mutex::default(),
            moon_strip: std::sync::OnceLock::new(),
        })
    }

    /// The minutes the night's Moon track is made of, in order.
    pub fn moon_spans(&self) -> Vec<Interval> {
        MoonTrack::spans(self.span()).unwrap_or_default()
    }

    /// Whether the Moon track is computed, or set.
    pub fn has_moon_track(&self) -> bool {
        self.track.get().is_some()
    }

    /// Set the Moon track from its cells, computed apart: one per span of
    /// [`NightSky::moon_spans`], in order. A track already set stays.
    pub fn set_moon_track(&self, cells: Vec<SkyMoonCell>) -> Result<(), NightError> {
        let track = MoonTrack::from_cells(self.span(), cells.into_iter().map(|c| c.0).collect())?;
        let _ = self.track.set(Ok(track));
        Ok(())
    }

    /// When the Sun is below `below_degrees`: the first such step and the
    /// end of the last, as a summary gives `dusk_ms` and `dawn_ms`. `None`
    /// when it never gets that low.
    pub fn darkness(&self, below_degrees: f64) -> Option<(u64, u64)> {
        let dark = |step: &&SkyStep| step.sun_altitude_degrees < below_degrees;
        let dusk = self.steps.iter().find(dark)?.t_ms;
        let dawn = self.steps.iter().rev().find(dark)?.t_ms + self.step_ms;
        Some((dusk, dawn))
    }

    /// A position as the site sees it at `t_ms`, one of the sky's
    /// instants, to the bit as [`crate::visibility::observe`] gives it.
    /// `None` when `t_ms` is not one of them.
    pub fn observe_at(
        &self,
        t_ms: u64,
        position: IcrsPosition,
    ) -> Option<Result<ObservedPosition, NightError>> {
        let offset = t_ms.checked_sub(self.noon_ms)?;
        if offset % self.step_ms != 0 {
            return None;
        }
        let step = self
            .steps
            .get(usize::try_from(offset / self.step_ms).ok()?)?;
        Some(step.observer.observe(position).map_err(NightError::from))
    }

    /// The Moon's altitude as the site sees it, parallax included, every
    /// [`MOON_STRIP_STEP_MS`] from the night's noon to the next: `None`
    /// where the ephemeris cannot place it. Each is
    /// [`crate::ephemeris::moon_at_site`] observed as
    /// [`crate::visibility::observe`] observes it, to the bit. Computed on
    /// first use and kept with the sky.
    pub fn moon_strip(&self) -> &[Option<f64>] {
        self.moon_strip.get_or_init(|| {
            (0..DAY_MS / MOON_STRIP_STEP_MS)
                .map(|step| {
                    let at = self.noon_ms + step * MOON_STRIP_STEP_MS;
                    let moon = crate::ephemeris::moon_at_site(at, self.site)?;
                    let seen = match self.observe_at(at, moon) {
                        Some(seen) => seen.ok()?,
                        None => crate::visibility::observe(
                            moon,
                            self.site,
                            planning_orientation(at, at),
                            at,
                        )
                        .ok()?,
                    };
                    Some(seen.altitude_degrees)
                })
                .collect()
        })
    }

    /// A target's windows outside the meridian pause over `span`, as
    /// [`meridian_windows`] finds them, kept for the next summary that
    /// asks.
    fn meridian_windows(
        &self,
        position: IcrsPosition,
        span: Interval,
        pause: MeridianExclusion,
    ) -> Result<Vec<Interval>, VisibilityError> {
        let key = (
            position.ra_degrees.to_bits(),
            position.dec_degrees.to_bits(),
            pause.before_ms,
            pause.after_ms,
            span.start_ms,
            span.end_ms,
        );
        if let Ok(kept) = self.meridian.lock()
            && let Some(found) = kept.get(&key)
        {
            return found.clone();
        }
        let found = meridian_windows(
            position,
            self.site,
            sky_orientation(self.noon_ms),
            span,
            pause,
        )
        .map(|found| found.windows);
        if let Ok(mut kept) = self.meridian.lock()
            && kept.len() < MAX_MERIDIAN_KEPT
        {
            kept.insert(key, found.clone());
        }
        found
    }

    fn span(&self) -> Interval {
        Interval {
            start_ms: self.noon_ms,
            end_ms: self.noon_ms + DAY_MS,
        }
    }

    fn moon_track(&self) -> Result<&MoonTrack, NightError> {
        self.track
            .get_or_init(|| MoonTrack::new(self.site, sky_orientation(self.noon_ms), self.span()))
            .as_ref()
            .map_err(|error| NightError::Visibility(*error))
    }
}

fn one_night(
    request: &NightRequest,
    index: u32,
    keep_samples: bool,
    sky: Option<&NightSky>,
) -> Result<NightCurve, NightError> {
    let first_noon = night_noon(request.start_ms, request.site.longitude_degrees)
        .ok_or(NightError::InvalidRequest)?;
    let step_hours = request.step_ms as f64 / 3_600_000.0;
    let noon = first_noon + u64::from(index) * DAY_MS;
    let next_noon = noon + DAY_MS;
    let computed;
    let sky = match sky {
        Some(sky) => {
            if sky.site != request.site || sky.noon_ms != noon || sky.step_ms != request.step_ms {
                return Err(NightError::InvalidRequest);
            }
            sky
        }
        None => {
            computed = NightSky::new(request.site, noon, request.step_ms)?;
            &computed
        }
    };
    let mut dusk = None;
    let mut dawn = None;
    let mut dark_hours = 0.0;
    let mut available_dark_hours = 0.0;
    let mut moon_hours = 0.0;
    let available: Option<Vec<Interval>> = request.available.as_deref().map(merged);
    let mut samples = Vec::new();
    let mut per_target: Vec<TargetNight> = request
        .targets
        .iter()
        .map(|target| TargetNight {
            id: target.id.clone(),
            hours_up: 0.0,
            hours_up_moon_down: 0.0,
            hours_up_available: 0.0,
            hours_up_moon_allowed: 0.0,
            hours_lost_to_meridian: 0.0,
            transit_ms: None,
            max_altitude_degrees: -90.0,
            min_moon_separation_degrees: 180.0,
        })
        .collect();
    // Allowed intervals around the meridian for each target, once per night.
    // The search widens the span by the pause on each side and allows at most
    // a day, so ask for the night less those margins; the margins fall around
    // local noon, where nothing is dark anyway.
    let paused =
        request.meridian_exclusion.before_ms > 0 || request.meridian_exclusion.after_ms > 0;
    let span = Interval {
        start_ms: noon
            .saturating_add(request.meridian_exclusion.before_ms)
            .saturating_add(1_000),
        end_ms: next_noon
            .saturating_sub(request.meridian_exclusion.after_ms)
            .saturating_sub(1_000),
    };
    // The window as an hour angle either side of the meridian; zero is off.
    let window_degrees = (request.meridian_window_ms > 0)
        .then(|| request.meridian_window_ms as f64 / 3_600_000.0 * SIDEREAL_DEGREES_PER_HOUR);
    let allowed_by_meridian: Vec<Option<Vec<Interval>>> = request
        .targets
        .iter()
        .map(|target| {
            if !paused {
                return Ok(None);
            }
            if span.start_ms >= span.end_ms {
                // A pause longer than the day blocks everything.
                return Ok(Some(vec![]));
            }
            sky.meridian_windows(target.position, span, request.meridian_exclusion)
                .map(Some)
        })
        .collect::<Result<_, VisibilityError>>()?;
    // Where each target's Moon rules allow imaging, met with availability:
    // one Moon track for the night, shared by every target and rule.
    let night_span = Interval {
        start_ms: noon,
        end_ms: next_noon,
    };
    let track = if request.targets.iter().any(|target| !target.moon.is_empty()) {
        Some(sky.moon_track()?)
    } else {
        None
    };
    let open = available.clone().unwrap_or_else(|| vec![night_span]);
    let moon_allowed: Vec<Option<Vec<Interval>>> = request
        .targets
        .iter()
        .map(|target| {
            let Some(track) = track.as_ref().filter(|_| !target.moon.is_empty()) else {
                return Ok(None);
            };
            let mut allowed = Vec::new();
            for policy in &target.moon {
                allowed.extend(track.windows(policy, target.position)?);
            }
            Ok(Some(intersection(&merged(&allowed), &open)))
        })
        .collect::<Result<_, VisibilityError>>()?;
    let mut last_hour_angle: Vec<Option<f64>> = vec![None; request.targets.len()];
    for step in &sky.steps {
        // One prepared observer per instant: the Earth's part of the sum is
        // the same for the Sun, the Moon and every target.
        let t = step.t_ms;
        let observer = &step.observer;
        let sun_altitude = step.sun_altitude_degrees;
        let dark = sun_altitude < request.dark_below_degrees;
        // Outside darkness only the drawing needs positions.
        let wanted = dark || keep_samples;
        let moon = step.moon;
        let moon_altitude = if wanted {
            observer.observe(moon)?.altitude_degrees
        } else {
            0.0
        };
        let moon_up = moon_altitude > 0.0;
        let share = if dark {
            available_share(available.as_deref(), t, request.step_ms)
        } else {
            0.0
        };
        if dark {
            if dusk.is_none() {
                dusk = Some(t);
            }
            dawn = Some(t + request.step_ms);
            dark_hours += step_hours;
            available_dark_hours += step_hours * share;
            if moon_up {
                moon_hours += step_hours;
            }
        }
        let mut target_samples = Vec::new();
        // Transit detection needs every sample, dark or not.
        for (index, (target, entry)) in request
            .targets
            .iter()
            .zip(per_target.iter_mut())
            .enumerate()
        {
            let observed = observer.observe(target.position)?;
            if let Some(previous) = last_hour_angle[index]
                && previous < 0.0
                && observed.hour_angle_degrees >= 0.0
                && observed.hour_angle_degrees - previous < 180.0
            {
                entry.transit_ms = Some(t);
            }
            last_hour_angle[index] = Some(observed.hour_angle_degrees);
            if !wanted {
                continue;
            }
            let allowed = altitude_allowed(
                &request.horizon,
                request.limits,
                observed.azimuth_degrees,
                observed.altitude_degrees,
            )? && window_degrees
                .is_none_or(|limit| observed.hour_angle_degrees.abs() <= limit);
            let meridian_blocked = allowed_by_meridian[index].as_ref().is_some_and(|windows| {
                span.start_ms >= span.end_ms
                    || (t >= span.start_ms
                        && t < span.end_ms
                        && !windows.iter().any(|w| t >= w.start_ms && t < w.end_ms))
            });
            if dark {
                entry.max_altitude_degrees =
                    entry.max_altitude_degrees.max(observed.altitude_degrees);
                entry.min_moon_separation_degrees = entry
                    .min_moon_separation_degrees
                    .min(separation_degrees(target.position, moon));
                if allowed && meridian_blocked {
                    entry.hours_lost_to_meridian += step_hours;
                } else if allowed {
                    entry.hours_up += step_hours;
                    entry.hours_up_available += step_hours * share;
                    entry.hours_up_moon_allowed += step_hours
                        * match &moon_allowed[index] {
                            Some(windows) => available_share(Some(windows), t, request.step_ms),
                            None => share,
                        };
                    if !moon_up {
                        entry.hours_up_moon_down += step_hours;
                    }
                }
            }
            if keep_samples {
                target_samples.push(TargetSample {
                    altitude_degrees: observed.altitude_degrees,
                    azimuth_degrees: observed.azimuth_degrees,
                    horizon_altitude_degrees: request
                        .horizon
                        .altitude(observed.azimuth_degrees)?
                        .map(|curve| curve + request.limits.horizon_offset_degrees),
                    allowed,
                    meridian_blocked,
                });
            }
        }
        if keep_samples {
            samples.push(Sample {
                t_ms: t,
                sun_altitude_degrees: sun_altitude,
                moon_altitude_degrees: moon_altitude,
                targets: target_samples,
            });
        }
    }
    let midnight = noon + DAY_MS / 2;
    Ok(NightCurve {
        night: Night {
            date: local_date(noon, request.site.longitude_degrees),
            noon_ms: noon,
            dusk_ms: dusk,
            dawn_ms: dawn,
            dark_hours,
            available_dark_hours,
            moon_illumination: moon_illumination(midnight).unwrap_or(0.0),
            moon_hours_up_in_dark: moon_hours,
            targets: per_target,
        },
        samples,
    })
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
                    moon: vec![],
                },
                NightTarget {
                    id: "south".into(),
                    position: IcrsPosition {
                        ra_degrees: 38.2,
                        dec_degrees: -75.0,
                    },
                    moon: vec![],
                },
            ],
            // 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
            start_ms: 1_790_294_400_000,
            nights: 2,
            step_ms: 300_000,
            dark_below_degrees: -12.0,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
            meridian_window_ms: 0,
            available: None,
        }
    }

    #[test]
    fn tonight_is_the_night_under_way_or_starting_this_evening() {
        let mut asked = request();
        asked.nights = 1;
        asked.targets.clear();
        // Los Angeles on 2026-10-05 at 16:00 and 22:00 PDT, and at 01:00 the
        // next morning, is still the night of the 5th.
        for start_ms in [1_791_241_200_000, 1_791_262_800_000, 1_791_273_600_000] {
            asked.start_ms = start_ms;
            let night = &night_preview(&asked).unwrap()[0];
            assert_eq!(night.date, "2026-10-05", "{start_ms}");
            assert!(night.noon_ms <= start_ms && start_ms < night.noon_ms + DAY_MS);
            assert!(night.dusk_ms.is_some_and(|dusk| dusk > night.noon_ms));
        }
        // From local mean noon, 12:53 PDT, the next night is tonight.
        asked.start_ms = 1_791_316_800_000; // 13:00 PDT on the 6th
        assert_eq!(night_preview(&asked).unwrap()[0].date, "2026-10-06");
    }

    #[test]
    fn a_night_date_spans_the_night_the_preview_reports() {
        let nights = night_preview(&request()).unwrap();
        for night in &nights {
            let date: NightDate = night.date.parse().unwrap();
            assert_eq!(date.to_string(), night.date);
            let span = date.span(request().site.longitude_degrees);
            assert_eq!(span.start_ms, night.noon_ms);
            assert_eq!(span.end_ms, night.noon_ms + DAY_MS);
            assert_eq!(NightDate::at(night.noon_ms + 3_600_000, -118.3), Some(date));
            assert_eq!(
                NightDate::at(night.noon_ms - 1, -118.3).and_then(NightDate::next),
                Some(date)
            );
        }
        for bad in [
            "2026-1-05",
            "2026-02-30",
            "1970-01-01",
            "2099-01-01",
            "tonight",
            "",
        ] {
            assert_eq!(bad.parse::<NightDate>(), Err(InvalidNightDate), "{bad}");
        }
        let json = serde_json::to_string(&"2026-11-03".parse::<NightDate>().unwrap()).unwrap();
        assert_eq!(json, "\"2026-11-03\"");
        assert!(serde_json::from_str::<NightDate>("\"2026-13-01\"").is_err());
    }

    #[test]
    fn moon_rules_count_the_hours_at_least_one_filter_may_shoot() {
        let open = night_preview(&request()).unwrap();
        let heart = &open[0].targets[0];
        assert_eq!(heart.hours_up_moon_allowed, heart.hours_up_available);
        // Late September 2026 has a full Moon: a strict broadband rule loses
        // the hours the Moon is up, a narrowband one keeps more of them.
        let strict = MoonPolicy {
            enabled: true,
            separation_degrees: 120.0,
            width_days: 14.0,
            ..MoonPolicy::default()
        };
        let lenient = MoonPolicy {
            enabled: true,
            separation_degrees: 20.0,
            width_days: 2.0,
            ..MoonPolicy::default()
        };
        let mut ruled = request();
        ruled.targets[0].moon = vec![strict.clone()];
        let blocked = night_preview(&ruled).unwrap()[0].targets[0].hours_up_moon_allowed;
        assert!(
            blocked < heart.hours_up - 1.0,
            "{blocked} of {}",
            heart.hours_up
        );
        ruled.targets[0].moon = vec![strict, lenient];
        let either = night_preview(&ruled).unwrap()[0].targets[0].hours_up_moon_allowed;
        assert!(
            either > blocked && either <= heart.hours_up + 1e-9,
            "{either}"
        );
        // Availability still clips what the Moon allows.
        ruled.available = Some(vec![]);
        assert_eq!(
            night_preview(&ruled).unwrap()[0].targets[0].hours_up_moon_allowed,
            0.0
        );
        let mut bad = request();
        bad.targets[0].moon = vec![MoonPolicy {
            width_days: 0.5,
            ..MoonPolicy::default()
        }];
        assert_eq!(night_preview(&bad), Err(NightError::InvalidRequest));
    }

    #[test]
    fn availability_clips_dark_hours_and_target_hours_to_its_windows() {
        let open = night_preview(&request()).unwrap();
        let first = &open[0];
        assert_eq!(first.available_dark_hours, first.dark_hours);
        assert_eq!(
            first.targets[0].hours_up_available,
            first.targets[0].hours_up
        );
        let dusk = first.dusk_ms.unwrap();
        let dawn = first.dawn_ms.unwrap();
        // Limited to an hour and a half from 07 minutes past a sample, so the
        // edges fall inside steps and count to the minute.
        let mut limited = request();
        limited.available = Some(vec![Interval {
            start_ms: dusk + 3_600_000 + 7 * 60_000,
            end_ms: dusk + 3_600_000 + 97 * 60_000,
        }]);
        let nights = night_preview(&limited).unwrap();
        assert!(
            (nights[0].available_dark_hours - 1.5).abs() < 1e-9,
            "{:?}",
            nights[0]
        );
        assert_eq!(nights[0].dark_hours, first.dark_hours);
        assert!(nights[0].targets[0].hours_up_available <= 1.5 + 1e-9);
        assert_eq!(nights[0].targets[0].hours_up, first.targets[0].hours_up);
        // The second night has no window at all: nothing is available.
        assert_eq!(nights[1].available_dark_hours, 0.0);
        assert_eq!(nights[1].targets[0].hours_up_available, 0.0);
        // A window across local midnight, overlapping a second one, counts once.
        let midnight = first.noon_ms + DAY_MS / 2;
        let mut across = request();
        across.nights = 1;
        across.available = Some(vec![
            Interval {
                start_ms: midnight - 3_600_000,
                end_ms: midnight + 3_600_000,
            },
            Interval {
                start_ms: midnight,
                end_ms: midnight + 1_800_000,
            },
        ]);
        let night = &night_preview(&across).unwrap()[0];
        assert!((night.available_dark_hours - 2.0).abs() < 1e-9, "{night:?}");
        assert!(
            (night.targets[0].hours_up_available - 2.0).abs() < 1e-9,
            "{night:?}"
        );
        // An unavailable night is an empty list, which is not the same as none.
        let mut closed = request();
        closed.available = Some(vec![]);
        let nights = night_preview(&closed).unwrap();
        assert!(nights
            .iter()
            .all(|n| n.available_dark_hours == 0.0 && n.dark_hours > 9.0));
        assert!(dawn > dusk);
        let mut bad = request();
        bad.available = Some(vec![Interval {
            start_ms: dusk,
            end_ms: dusk,
        }]);
        assert_eq!(night_preview(&bad), Err(NightError::InvalidRequest));
    }

    #[test]
    fn a_meridian_window_keeps_only_the_hours_around_transit() {
        let open = night_preview(&request()).unwrap();
        let mut windowed = request();
        windowed.meridian_window_ms = 3_600_000;
        let nights = night_preview(&windowed).unwrap();
        let heart = &nights[0].targets[0];
        assert!(open[0].targets[0].hours_up > 7.0);
        assert!(heart.hours_up > 1.8 && heart.hours_up <= 2.1, "{heart:?}");
        assert_eq!(heart.hours_lost_to_meridian, 0.0);
        let curve = night_curve(&windowed, 0).unwrap();
        let transit = heart.transit_ms.unwrap();
        for sample in &curve.samples {
            if sample.t_ms.abs_diff(transit) > 3_700_000 {
                assert!(!sample.targets[0].allowed, "{}", sample.t_ms);
            }
        }
        let mut wide = request();
        wide.meridian_window_ms = DAY_MS;
        assert_eq!(night_preview(&wide), Err(NightError::InvalidRequest));
    }

    #[test]
    fn a_start_outside_the_supported_years_is_refused_without_overflow() {
        for start_ms in [0, LATEST_START_MS + 1, u64::MAX] {
            let mut far = request();
            far.start_ms = start_ms;
            assert_eq!(night_preview(&far), Err(NightError::InvalidRequest));
        }
        let mut last = request();
        last.start_ms = LATEST_START_MS;
        last.nights = MAX_NIGHTS;
        assert_eq!(night_preview(&last).unwrap().len(), MAX_NIGHTS as usize);
        // A pause longer than the day blocks the target rather than overflowing.
        let mut paused = request();
        paused.meridian_exclusion = MeridianExclusion {
            before_ms: u64::MAX,
            after_ms: u64::MAX,
        };
        let nights = night_preview(&paused).unwrap();
        assert_eq!(nights[0].targets[0].hours_up, 0.0);
        assert!(nights[0].targets[0].hours_lost_to_meridian > 7.0);
    }

    #[test]
    fn a_meridian_pause_takes_dark_hours_away_and_the_transit_is_found() {
        let open = night_preview(&request()).unwrap();
        let mut paused = request();
        paused.meridian_exclusion = MeridianExclusion {
            before_ms: 3_600_000,
            after_ms: 1_800_000,
        };
        let nights = night_preview(&paused).unwrap();
        let heart_open = &open[0].targets[0];
        let heart = &nights[0].targets[0];
        assert_eq!(heart_open.hours_lost_to_meridian, 0.0);
        assert!(
            heart.hours_lost_to_meridian > 1.0 && heart.hours_lost_to_meridian <= 1.6,
            "{heart:?}"
        );
        assert!((heart.hours_up + heart.hours_lost_to_meridian - heart_open.hours_up).abs() < 0.01);
        // At the equinox sidereal time matches UT at Greenwich, so RA 2h33m
        // transits Los Angeles near 10:10 UTC, about fourteen hours after its
        // local noon.
        let transit = heart.transit_ms.expect("transit");
        assert!(
            transit > nights[0].noon_ms + 13 * 3_600_000
                && transit < nights[0].noon_ms + 15 * 3_600_000,
            "{transit}"
        );
        assert_eq!(heart.transit_ms, heart_open.transit_ms);
        let curve = night_curve(&paused, 0).unwrap();
        assert!(curve.samples.iter().any(|s| s.targets[0].meridian_blocked));
        assert!(curve.samples.iter().any(|s| !s.targets[0].meridian_blocked));
    }

    #[test]
    fn a_los_angeles_september_night_is_about_ten_dark_hours_with_a_full_moon() {
        let nights = night_preview(&request()).unwrap();
        assert_eq!(nights.len(), 2);
        let first = &nights[0];
        // The night that starts that evening, local time.
        assert_eq!(first.date, "2026-09-24");
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
        assert_eq!(nights[1].date, "2026-09-25");
    }

    #[test]
    fn a_curve_samples_the_whole_night_and_carries_the_horizon_at_each_azimuth() {
        let mut walled = request();
        walled.horizon = Horizon::Custom {
            points: vec![
                crate::visibility::HorizonPoint {
                    azimuth_degrees: 0.0,
                    altitude_degrees: 10.0,
                },
                crate::visibility::HorizonPoint {
                    azimuth_degrees: 180.0,
                    altitude_degrees: 40.0,
                },
                crate::visibility::HorizonPoint {
                    azimuth_degrees: 360.0,
                    altitude_degrees: 10.0,
                },
            ],
        };
        let curve = night_curve(&walled, 0).unwrap();
        assert_eq!(curve.samples.len(), 288);
        assert_eq!(curve.night.date, "2026-09-24");
        assert!(curve.samples.iter().any(|s| s.sun_altitude_degrees > 0.0));
        assert!(curve.samples.iter().any(|s| s.sun_altitude_degrees < -18.0));
        let heart: Vec<&TargetSample> = curve.samples.iter().map(|s| &s.targets[0]).collect();
        assert!(heart.iter().all(|t| t.horizon_altitude_degrees.is_some()));
        assert!(heart.iter().any(|t| t.allowed) && heart.iter().any(|t| !t.allowed));
        // A project's horizon offset lifts the drawn curve and the limit.
        let mut raised = walled.clone();
        raised.limits.horizon_offset_degrees = 30.0;
        let lifted = night_curve(&raised, 0).unwrap();
        for (low, high) in curve.samples.iter().zip(&lifted.samples) {
            let low = low.targets[0].horizon_altitude_degrees.unwrap();
            let high = high.targets[0].horizon_altitude_degrees.unwrap();
            assert!((high - low - 30.0).abs() < 1e-9);
        }
        assert!(lifted.night.targets[0].hours_up < curve.night.targets[0].hours_up);
        // The summary of the curve is the summary of the preview.
        let preview = night_preview(&walled).unwrap();
        assert_eq!(curve.night, preview[0]);
        assert_eq!(
            night_curve(&walled, 2).unwrap_err(),
            NightError::InvalidRequest
        );
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

    /// The night's sums before skies were shared: kept here to show the
    /// shared sky changes nothing.
    mod before {
        use super::super::*;

        fn one_night_before(
            request: &NightRequest,
            index: u32,
            keep_samples: bool,
        ) -> Result<NightCurve, NightError> {
            let first_noon = night_noon(request.start_ms, request.site.longitude_degrees)
                .ok_or(NightError::InvalidRequest)?;
            let end = first_noon + u64::from(request.nights) * DAY_MS;
            let orientation = planning_orientation(first_noon, end);
            let step_hours = request.step_ms as f64 / 3_600_000.0;
            let noon = first_noon + u64::from(index) * DAY_MS;
            let next_noon = noon + DAY_MS;
            let mut dusk = None;
            let mut dawn = None;
            let mut dark_hours = 0.0;
            let mut available_dark_hours = 0.0;
            let mut moon_hours = 0.0;
            let available: Option<Vec<Interval>> = request.available.as_deref().map(merged);
            let mut samples = Vec::new();
            let mut per_target: Vec<TargetNight> = request
                .targets
                .iter()
                .map(|target| TargetNight {
                    id: target.id.clone(),
                    hours_up: 0.0,
                    hours_up_moon_down: 0.0,
                    hours_up_available: 0.0,
                    hours_up_moon_allowed: 0.0,
                    hours_lost_to_meridian: 0.0,
                    transit_ms: None,
                    max_altitude_degrees: -90.0,
                    min_moon_separation_degrees: 180.0,
                })
                .collect();
            // Allowed intervals around the meridian for each target, once per night.
            // The search widens the span by the pause on each side and allows at most
            // a day, so ask for the night less those margins; the margins fall around
            // local noon, where nothing is dark anyway.
            let paused =
                request.meridian_exclusion.before_ms > 0 || request.meridian_exclusion.after_ms > 0;
            let span = Interval {
                start_ms: noon
                    .saturating_add(request.meridian_exclusion.before_ms)
                    .saturating_add(1_000),
                end_ms: next_noon
                    .saturating_sub(request.meridian_exclusion.after_ms)
                    .saturating_sub(1_000),
            };
            // The window as an hour angle either side of the meridian; zero is off.
            let window_degrees = (request.meridian_window_ms > 0).then(|| {
                request.meridian_window_ms as f64 / 3_600_000.0 * SIDEREAL_DEGREES_PER_HOUR
            });
            let allowed_by_meridian: Vec<Option<Vec<Interval>>> = request
                .targets
                .iter()
                .map(|target| {
                    if !paused {
                        return Ok(None);
                    }
                    if span.start_ms >= span.end_ms {
                        // A pause longer than the day blocks everything.
                        return Ok(Some(vec![]));
                    }
                    meridian_windows(
                        target.position,
                        request.site,
                        orientation,
                        span,
                        request.meridian_exclusion,
                    )
                    .map(|found| Some(found.windows))
                })
                .collect::<Result<_, VisibilityError>>()?;
            // Where each target's Moon rules allow imaging, met with availability:
            // one Moon track for the night, shared by every target and rule.
            let night_span = Interval {
                start_ms: noon,
                end_ms: next_noon,
            };
            let track = if request.targets.iter().any(|target| !target.moon.is_empty()) {
                Some(MoonTrack::new(request.site, orientation, night_span)?)
            } else {
                None
            };
            let open = available.clone().unwrap_or_else(|| vec![night_span]);
            let moon_allowed: Vec<Option<Vec<Interval>>> = request
                .targets
                .iter()
                .map(|target| {
                    let Some(track) = track.as_ref().filter(|_| !target.moon.is_empty()) else {
                        return Ok(None);
                    };
                    let mut allowed = Vec::new();
                    for policy in &target.moon {
                        allowed.extend(track.windows(policy, target.position)?);
                    }
                    Ok(Some(intersection(&merged(&allowed), &open)))
                })
                .collect::<Result<_, VisibilityError>>()?;
            let mut last_hour_angle: Vec<Option<f64>> = vec![None; request.targets.len()];
            let mut t = noon;
            while t < next_noon {
                // One prepared observer per instant: the Earth's part of the sum is
                // the same for the Sun, the Moon and every target.
                let observer = Observer::at(request.site, orientation, t)?;
                let sun = sun_position(t).ok_or(VisibilityError::AstronomyUnavailable)?;
                let sun_altitude = observer.observe(sun)?.altitude_degrees;
                let dark = sun_altitude < request.dark_below_degrees;
                // Outside darkness only the drawing needs positions.
                let wanted = dark || keep_samples;
                let moon = moon_position(t);
                let moon_altitude = if wanted {
                    observer.observe(moon)?.altitude_degrees
                } else {
                    0.0
                };
                let moon_up = moon_altitude > 0.0;
                let share = if dark {
                    available_share(available.as_deref(), t, request.step_ms)
                } else {
                    0.0
                };
                if dark {
                    if dusk.is_none() {
                        dusk = Some(t);
                    }
                    dawn = Some(t + request.step_ms);
                    dark_hours += step_hours;
                    available_dark_hours += step_hours * share;
                    if moon_up {
                        moon_hours += step_hours;
                    }
                }
                let mut target_samples = Vec::new();
                // Transit detection needs every sample, dark or not.
                for (index, (target, entry)) in request
                    .targets
                    .iter()
                    .zip(per_target.iter_mut())
                    .enumerate()
                {
                    let observed = observer.observe(target.position)?;
                    if let Some(previous) = last_hour_angle[index]
                        && previous < 0.0
                        && observed.hour_angle_degrees >= 0.0
                        && observed.hour_angle_degrees - previous < 180.0
                    {
                        entry.transit_ms = Some(t);
                    }
                    last_hour_angle[index] = Some(observed.hour_angle_degrees);
                    if !wanted {
                        continue;
                    }
                    let allowed = altitude_allowed(
                        &request.horizon,
                        request.limits,
                        observed.azimuth_degrees,
                        observed.altitude_degrees,
                    )? && window_degrees
                        .is_none_or(|limit| observed.hour_angle_degrees.abs() <= limit);
                    let meridian_blocked =
                        allowed_by_meridian[index].as_ref().is_some_and(|windows| {
                            span.start_ms >= span.end_ms
                                || (t >= span.start_ms
                                    && t < span.end_ms
                                    && !windows.iter().any(|w| t >= w.start_ms && t < w.end_ms))
                        });
                    if dark {
                        entry.max_altitude_degrees =
                            entry.max_altitude_degrees.max(observed.altitude_degrees);
                        entry.min_moon_separation_degrees = entry
                            .min_moon_separation_degrees
                            .min(separation_degrees(target.position, moon));
                        if allowed && meridian_blocked {
                            entry.hours_lost_to_meridian += step_hours;
                        } else if allowed {
                            entry.hours_up += step_hours;
                            entry.hours_up_available += step_hours * share;
                            entry.hours_up_moon_allowed += step_hours
                                * match &moon_allowed[index] {
                                    Some(windows) => {
                                        available_share(Some(windows), t, request.step_ms)
                                    }
                                    None => share,
                                };
                            if !moon_up {
                                entry.hours_up_moon_down += step_hours;
                            }
                        }
                    }
                    if keep_samples {
                        target_samples.push(TargetSample {
                            altitude_degrees: observed.altitude_degrees,
                            azimuth_degrees: observed.azimuth_degrees,
                            horizon_altitude_degrees: request
                                .horizon
                                .altitude(observed.azimuth_degrees)?
                                .map(|curve| curve + request.limits.horizon_offset_degrees),
                            allowed,
                            meridian_blocked,
                        });
                    }
                }
                if keep_samples {
                    samples.push(Sample {
                        t_ms: t,
                        sun_altitude_degrees: sun_altitude,
                        moon_altitude_degrees: moon_altitude,
                        targets: target_samples,
                    });
                }
                t += request.step_ms;
            }
            let midnight = noon + DAY_MS / 2;
            Ok(NightCurve {
                night: Night {
                    date: local_date(noon, request.site.longitude_degrees),
                    noon_ms: noon,
                    dusk_ms: dusk,
                    dawn_ms: dawn,
                    dark_hours,
                    available_dark_hours,
                    moon_illumination: moon_illumination(midnight).unwrap_or(0.0),
                    moon_hours_up_in_dark: moon_hours,
                    targets: per_target,
                },
                samples,
            })
        }

        fn rich() -> NightRequest {
            let mut request = super::request();
            request.horizon = Horizon::Custom {
                points: vec![
                    crate::visibility::HorizonPoint {
                        azimuth_degrees: 0.0,
                        altitude_degrees: 10.0,
                    },
                    crate::visibility::HorizonPoint {
                        azimuth_degrees: 180.0,
                        altitude_degrees: 40.0,
                    },
                    crate::visibility::HorizonPoint {
                        azimuth_degrees: 360.0,
                        altitude_degrees: 10.0,
                    },
                ],
            };
            request.limits.horizon_offset_degrees = 3.0;
            request.nights = 4;
            request.meridian_exclusion = MeridianExclusion {
                before_ms: 600_000,
                after_ms: 900_000,
            };
            request.meridian_window_ms = 3 * 3_600_000;
            let strict = MoonPolicy {
                enabled: true,
                separation_degrees: 120.0,
                ..Default::default()
            };
            let loose = MoonPolicy {
                enabled: true,
                separation_degrees: 30.0,
                width_days: 3.0,
                ..Default::default()
            };
            request.targets = (0..6)
                .map(|i| NightTarget {
                    id: i.to_string(),
                    position: IcrsPosition {
                        ra_degrees: 15.0 + 57.0 * f64::from(i),
                        dec_degrees: -20.0 + 15.0 * f64::from(i),
                    },
                    moon: match i % 3 {
                        0 => vec![],
                        1 => vec![strict.clone()],
                        _ => vec![strict.clone(), loose.clone()],
                    },
                })
                .collect();
            let noon = request.noon_ms(0).unwrap();
            request.available = Some(vec![
                Interval {
                    start_ms: noon + 7 * 3_600_000,
                    end_ms: noon + 11 * 3_600_000 + 120_000,
                },
                Interval {
                    start_ms: noon + 10 * 3_600_000,
                    end_ms: noon + 30 * 3_600_000,
                },
                Interval {
                    start_ms: noon + 50 * 3_600_000,
                    end_ms: noon + 90 * 3_600_000,
                },
            ]);
            request
        }

        #[test]
        fn every_night_and_curve_is_what_it_was_before_skies_were_shared() {
            let mut requests = vec![super::request(), rich()];
            let mut dawn = rich();
            dawn.dark_below_degrees = -6.0;
            dawn.step_ms = 600_000;
            requests.push(dawn);
            let mut arctic = rich();
            arctic.site.latitude_degrees = 66.0;
            arctic.start_ms = 1_781_740_800_000;
            requests.push(arctic);
            for request in &requests {
                for index in 0..request.nights {
                    let before = one_night_before(request, index, true).unwrap();
                    assert_eq!(night_curve(request, index).unwrap(), before);
                    assert_eq!(night_summary(request, index).unwrap(), before.night);
                    // Its sky put together from steps and Moon cells
                    // computed apart, in reverse, as a pool would.
                    let noon = request.noon_ms(index).unwrap();
                    let mut steps: Vec<SkyStep> = NightSky::instants(noon, request.step_ms)
                        .unwrap()
                        .into_iter()
                        .rev()
                        .map(|t| SkyStep::at(request.site, noon, t).unwrap())
                        .collect();
                    steps.reverse();
                    let sky =
                        NightSky::from_steps(request.site, noon, request.step_ms, steps).unwrap();
                    let cells = sky
                        .moon_spans()
                        .into_iter()
                        .map(|span| SkyMoonCell::at(request.site, noon, span).unwrap())
                        .collect();
                    sky.set_moon_track(cells).unwrap();
                    assert_eq!(
                        night_summary_under(request, index, &sky).unwrap(),
                        before.night
                    );
                    assert_eq!(night_curve_under(request, index, &sky).unwrap(), before);
                    // Another night's sky, or another step's, is refused.
                    if index > 0 {
                        assert_eq!(
                            night_summary_under(request, index - 1, &sky),
                            Err(NightError::InvalidRequest)
                        );
                    }
                }
            }
            // A sky's darkness is its summary's dusk and dawn.
            let request = super::request();
            let night = night_summary(&request, 0).unwrap();
            let sky =
                NightSky::new(request.site, request.noon_ms(0).unwrap(), request.step_ms).unwrap();
            assert_eq!(sky.darkness(-12.0), night.dusk_ms.zip(night.dawn_ms));
            assert!(!sky.has_moon_track());
        }
    }
}
