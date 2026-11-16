//! How long a place in the sky stays in view on one night: its longest
//! unbroken run of dark samples inside the horizon and the altitude limits.
//! A night's hours up are a sum, which a tree in the way splits without
//! shortening; a rig that needs half an hour on a target needs it in one
//! piece.
//!
//! # The shortcut, and why it gives the same answers
//!
//! The rule is exact: at each dark sample, the place is observed from the
//! site ([`crate::visibility::Observer::observe`]) and its direction is
//! judged by [`AltitudeTest::allowed`]. An observation costs about 0.4 µs,
//! and a map of the sky asks for tens of millions.
//!
//! Over one night a place barely moves among the stars as the site sees
//! them: it keeps its declination and its hour angle grows as the Earth
//! turns. So a place is observed exactly once a night, at a sample in the
//! middle of the dark, and its direction at every other sample is worked
//! out from that with a few multiplications: its declination kept, its
//! hour angle moved on by as much as the Earth turned between the two
//! samples. The turn is the angle each sample's own observer measures
//! hour angles from, so whatever the clock does (UT1, a leap second) is
//! in it.
//!
//! That direction is not exact. What it leaves out changes over a night
//! by about an arcsecond: annual aberration turns with the Earth's
//! velocity (under 0.2″ in 12 hours), diurnal aberration depends on the
//! hour angle (up to 0.64″), and precession and nutation move the pole
//! (under 0.1″). Light bent by the Sun is steady over a night. The tests
//! measure it over sites from pole to pole and the years 1972 to 2098,
//! find half an arcsecond, and hold it under a tenth of [`SURE_DEGREES`].
//!
//! So the shortcut never decides a sample it could get wrong. It answers
//! only when every direction within [`SURE_DEGREES`] (36″) of the one it
//! worked out gets the same answer from [`AltitudeTest`]: clear of the
//! minimum, the horizon curve and the maximum by that much, in altitude
//! and across the curve in azimuth. Any other sample, a few in a thousand,
//! is observed exactly as before. The answers are therefore the plain
//! rule's own, and [`DarkRuns::plain`] keeps the plain rule for the tests
//! to hold the shortcut to.
//!
//! Over a run of nights ([`place_over`]) a place is not even observed once
//! a night. Where it sits among the stars drifts by a fraction of an
//! arcsecond a day, smoothly: aberration follows the year, precession is
//! steady, and the fortnightly nutation is 0.2″. So it is observed on
//! nights at most [`ANCHOR_DAYS`] apart and placed on the nights between
//! by a straight line from one to the next, which adds under half an
//! arcsecond. A place the Sun passes within a degree of in those days,
//! which is in daylight then, may be up to 2″ off for the bending of its
//! light. The tests hold the whole of it under the same tenth of the
//! margin.

use super::{NightError, NightSky};
use crate::visibility::{AltitudeLimits, AltitudeTest, Horizon, IcrsPosition};

/// How far, in degrees on the sky, a sample's worked-out direction must be
/// from any change of answer before the shortcut answers for it. The
/// worked-out direction is within a few arcseconds of the exact one.
const SURE_DEGREES: f64 = 0.01;
/// The most days between two nights a place is observed on, for the nights
/// between to be placed from them.
const ANCHOR_DAYS: u64 = 8;
/// The sine of 89°. Higher than that the margin on the sky spans more
/// than half a degree of azimuth, and the curve is asked directly.
const STEEP_SINE: f64 = 0.999_847_695_156_391_3;

/// One night's sky under one horizon and one set of limits, ready to
/// measure many positions: the dark samples are found once, and the
/// horizon and limits are validated once.
pub struct DarkRuns<'a> {
    sky: &'a NightSky,
    test: AltitudeTest<'a>,
    /// The sky's dark samples, as runs of consecutive samples: one run on
    /// an ordinary night, none when it never gets dark.
    blocks: Vec<std::ops::Range<usize>>,
    /// The shortcut; `None` for the plain rule, or with no dark samples.
    quick: Option<Quick>,
    /// How many samples the shortcut answered, and how many were observed.
    #[cfg(test)]
    counted: (
        std::sync::atomic::AtomicUsize,
        std::sync::atomic::AtomicUsize,
    ),
}

/// What the shortcut knows of a night: the site, how far the sky has
/// turned at each sample, and the altitudes that settle a sample whatever
/// its azimuth.
struct Quick {
    sin_latitude: f64,
    cos_latitude: f64,
    /// The sample each place is observed exactly at.
    reference: usize,
    /// How far the Earth had turned at that sample, in radians, and the
    /// sine and cosine of it.
    turned: f64,
    sin_turned: f64,
    cos_turned: f64,
    /// At each sample, the cosine and sine of the hour angle gained since
    /// the reference sample. Only dark samples are read.
    turns: Vec<(f64, f64)>,
    /// Sines of altitude. Under `out_below` or over `out_above` a place is
    /// out of view at any azimuth; over `in_above` and under `in_below` it
    /// is in view at any azimuth. Each is a margin clear of its limit.
    out_below: f64,
    in_above: f64,
    in_below: f64,
    out_above: f64,
    /// With a horizon curve, the same two sines for each whole degree of
    /// azimuth: under the first a place there is under the curve, over the
    /// second it is over it. Each takes in the curve a degree either side,
    /// which is more than the margin spans below [`STEEP_SINE`].
    by_degree: Option<Vec<(f64, f64)>>,
}

/// Where a place is among the turning stars at the reference sample.
#[derive(Clone, Copy)]
struct Track {
    sin_declination: f64,
    cos_declination: f64,
    sin_hour: f64,
    cos_hour: f64,
}

impl<'a> DarkRuns<'a> {
    /// `sky` judged with the Sun below `dark_below_degrees` as dark, and
    /// [`crate::visibility::altitude_allowed`] under `horizon` and `limits`
    /// as in view, as a night's summary judges them.
    pub fn new(
        sky: &'a NightSky,
        horizon: &'a Horizon,
        limits: AltitudeLimits,
        dark_below_degrees: f64,
    ) -> Result<Self, NightError> {
        let mut runs = Self::plain(sky, horizon, limits, dark_below_degrees)?;
        runs.quick = Quick::new(sky, &runs.test, &runs.blocks)?;
        Ok(runs)
    }

    /// [`DarkRuns::new`] without the shortcut: every sample is observed.
    /// The answers are the same and take many times longer; the tests
    /// compare the two.
    pub fn plain(
        sky: &'a NightSky,
        horizon: &'a Horizon,
        limits: AltitudeLimits,
        dark_below_degrees: f64,
    ) -> Result<Self, NightError> {
        if !dark_below_degrees.is_finite() {
            return Err(NightError::InvalidRequest);
        }
        let mut blocks: Vec<std::ops::Range<usize>> = Vec::new();
        for (index, step) in sky.steps.iter().enumerate() {
            if step.sun_altitude_degrees >= dark_below_degrees {
                continue;
            }
            match blocks.last_mut() {
                Some(block) if block.end == index => block.end = index + 1,
                _ => blocks.push(index..index + 1),
            }
        }
        Ok(Self {
            sky,
            test: AltitudeTest::new(horizon, limits)?,
            blocks,
            quick: None,
            #[cfg(test)]
            counted: Default::default(),
        })
    }

    /// The sample step, in milliseconds.
    pub fn step_ms(&self) -> u64 {
        self.sky.step_ms
    }

    /// Whether `position` is in view at sample `index`. `track` is where
    /// the shortcut placed it, found at the first sample asked about.
    fn in_view(
        &self,
        index: usize,
        position: IcrsPosition,
        track: &mut Option<Track>,
    ) -> Result<bool, NightError> {
        if let Some(quick) = &self.quick {
            let track = match track {
                Some(track) => *track,
                None => *track.insert(quick.track(self.sky, position)?),
            };
            if let Some(answer) = quick.answer(&self.test, index, track) {
                #[cfg(test)]
                self.counted
                    .0
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                return Ok(answer);
            }
        }
        #[cfg(test)]
        self.counted
            .1
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let seen = self.sky.steps[index].observer.observe(position)?;
        Ok(self
            .test
            .allowed(seen.azimuth_degrees, seen.altitude_degrees)?)
    }

    /// The longest run of consecutive dark samples with `position` in
    /// view, in samples. A sample that is not dark, or has the position
    /// out of view, ends a run.
    pub fn longest_steps(&self, position: IcrsPosition) -> Result<u32, NightError> {
        let mut longest = 0;
        let mut track = None;
        for block in &self.blocks {
            let mut run = 0;
            for index in block.clone() {
                if self.in_view(index, position, &mut track)? {
                    run += 1;
                    longest = longest.max(run);
                } else {
                    run = 0;
                }
            }
        }
        Ok(longest)
    }

    /// Whether some run is at least `steps` samples long: the same answer
    /// as comparing [`DarkRuns::longest_steps`], for a fraction of the
    /// work. A run of `steps` samples covers exactly one of every `steps`
    /// samples, so only those are looked at first, and a run is measured
    /// only around one that is in view, and only until it is long enough.
    pub fn reaches(&self, position: IcrsPosition, steps: u32) -> Result<bool, NightError> {
        self.reaches_from(position, steps, None)
    }

    /// [`DarkRuns::reaches`] for `position` as [`place_over`] placed it:
    /// `night` is this night's place in the nights it was placed over. The
    /// answer is the same; the place is not observed again to find it.
    pub fn reaches_placed(
        &self,
        position: IcrsPosition,
        steps: u32,
        placed: &Placed,
        night: usize,
    ) -> Result<bool, NightError> {
        self.reaches_from(position, steps, placed.0.get(night).copied().flatten())
    }

    fn reaches_from(
        &self,
        position: IcrsPosition,
        steps: u32,
        mut track: Option<Track>,
    ) -> Result<bool, NightError> {
        let needed = steps as usize;
        if needed == 0 {
            return Ok(true);
        }
        for block in &self.blocks {
            let mut probe = block.start + needed - 1;
            while probe < block.end {
                if self.in_view(probe, position, &mut track)? {
                    let mut run = 1;
                    let mut before = probe;
                    while run < needed && before > block.start {
                        before -= 1;
                        if !self.in_view(before, position, &mut track)? {
                            break;
                        }
                        run += 1;
                    }
                    let mut after = probe + 1;
                    while run < needed && after < block.end {
                        if !self.in_view(after, position, &mut track)? {
                            break;
                        }
                        run += 1;
                        after += 1;
                    }
                    if run >= needed {
                        return Ok(true);
                    }
                }
                probe += needed;
            }
        }
        Ok(false)
    }

    /// [`DarkRuns::longest_steps`] as time: each sample counts for one
    /// step, as it does in a night's hours up.
    pub fn longest_ms(&self, position: IcrsPosition) -> Result<u64, NightError> {
        Ok(u64::from(self.longest_steps(position)?) * self.sky.step_ms)
    }
}

/// Where one place is among the turning stars on each of several nights,
/// from [`place_over`]. A night it holds nothing for places the position
/// itself, with one observation, as [`DarkRuns::reaches`] does.
pub struct Placed(Vec<Option<Track>>);

/// A place among the stars as a unit vector: towards where hour angles
/// are measured from before the Earth's turn, a quarter turn west of
/// that, and to the pole.
type Among = [f64; 3];

/// Place `position` on each of `nights`, which are one site's nights in
/// order. It is observed on the first and last dark nights and on enough
/// between that no two are more than [`ANCHOR_DAYS`] apart, and placed on
/// the others by a straight line between the two it lies between: see the
/// module's account of why that is within the shortcut's margin. Nights
/// that are not one site's in order, or lie in a longer gap, are left for
/// [`DarkRuns::reaches_placed`] to observe.
pub fn place_over(nights: &[DarkRuns<'_>], position: IcrsPosition) -> Result<Placed, NightError> {
    let mut tracks: Vec<Option<Track>> = vec![None; nights.len()];
    let in_order = nights.windows(2).all(|pair| {
        pair[0].sky.site == pair[1].sky.site
            && pair[0].sky.step_ms == pair[1].sky.step_ms
            && pair[0].sky.noon_ms < pair[1].sky.noon_ms
    });
    if !in_order {
        return Ok(Placed(tracks));
    }
    let reach = ANCHOR_DAYS * super::DAY_MS;
    let noon = |night: usize| nights[night].sky.noon_ms;
    // The nights with any dark, which are the nights with a shortcut.
    let dark: Vec<(usize, &Quick)> = nights
        .iter()
        .enumerate()
        .filter_map(|(night, runs)| Some((night, runs.quick.as_ref()?)))
        .collect();
    let observed = |at: usize| -> Result<Among, NightError> {
        let (night, quick) = dark[at];
        quick.among(nights[night].sky, position)
    };
    let mut from = 0;
    while from < dark.len() {
        let here = observed(from)?;
        tracks[dark[from].0] = Some(dark[from].1.track_among(here));
        // The furthest dark night still within reach of this one.
        let to = (from + 1..dark.len())
            .take_while(|at| noon(dark[*at].0) - noon(dark[from].0) <= reach)
            .last();
        let Some(to) = to else {
            // None within reach: the next dark night is observed afresh.
            from += 1;
            continue;
        };
        let there = observed(to)?;
        let span = (noon(dark[to].0) - noon(dark[from].0)) as f64;
        for at in from + 1..to {
            let (night, quick) = dark[at];
            let along = (noon(night) - noon(dark[from].0)) as f64 / span;
            let between = [
                here[0] + (there[0] - here[0]) * along,
                here[1] + (there[1] - here[1]) * along,
                here[2] + (there[2] - here[2]) * along,
            ];
            tracks[night] = Some(quick.track_among(between));
        }
        if to == dark.len() - 1 {
            tracks[dark[to].0] = Some(dark[to].1.track_among(there));
            break;
        }
        from = to;
    }
    Ok(Placed(tracks))
}

impl Quick {
    /// The shortcut for one night; `None` when it never gets dark.
    fn new(
        sky: &NightSky,
        test: &AltitudeTest<'_>,
        blocks: &[std::ops::Range<usize>],
    ) -> Result<Option<Self>, NightError> {
        let (Some(first), Some(last)) = (blocks.first(), blocks.last()) else {
            return Ok(None);
        };
        // The middle of the dark: no sample is more than half of it away.
        let reference = (first.start + last.end - 1) / 2;
        // How far the Earth has turned since the reference sample, from
        // the angle each sample's observer itself measures hour angles from.
        let at_reference = sky.steps[reference].observer.rotation();
        let mut turns = vec![(f64::NAN, f64::NAN); sky.steps.len()];
        for index in blocks.iter().flat_map(|block| block.clone()) {
            let (sin, cos) = (sky.steps[index].observer.rotation() - at_reference).sin_cos();
            turns[index] = (cos, sin);
        }
        let (lowest_minimum, highest_minimum, maximum) = test.extent();
        // The sine of an altitude, or `beyond` when the altitude is off
        // the sky, so that no sine compares past it.
        let sine = |altitude: f64, beyond: f64| {
            if altitude.abs() < 90.0 {
                altitude.to_radians().sin()
            } else {
                beyond
            }
        };
        let latitude = sky.site.latitude_degrees.to_radians();
        Ok(Some(Self {
            sin_latitude: latitude.sin(),
            cos_latitude: latitude.cos(),
            reference,
            turned: at_reference,
            sin_turned: at_reference.sin(),
            cos_turned: at_reference.cos(),
            turns,
            out_below: sine(lowest_minimum - SURE_DEGREES, -2.0),
            in_above: sine(highest_minimum + SURE_DEGREES, 2.0),
            in_below: sine(maximum - SURE_DEGREES, -2.0),
            out_above: sine(maximum + SURE_DEGREES, 2.0),
            by_degree: test.minimum_by_degree().map(|spans| {
                spans
                    .into_iter()
                    .map(|(lowest, highest)| {
                        (
                            sine(lowest - SURE_DEGREES, -2.0),
                            sine(highest + SURE_DEGREES, 2.0),
                        )
                    })
                    .collect()
            }),
        }))
    }

    /// Where `position` is among the turning stars: one exact observation.
    fn track(&self, sky: &NightSky, position: IcrsPosition) -> Result<Track, NightError> {
        let (seen, declination) = sky.steps[self.reference]
            .observer
            .observe_with_declination(position)?;
        let (sin_declination, cos_declination) = declination.to_radians().sin_cos();
        let (sin_hour, cos_hour) = seen.hour_angle_degrees.to_radians().sin_cos();
        Ok(Track {
            sin_declination,
            cos_declination,
            sin_hour,
            cos_hour,
        })
    }

    /// Where `position` is among the stars at the reference sample, from
    /// one exact observation: its hour angle less the Earth's turn, which
    /// is where it sits around the sky, and its declination.
    fn among(&self, sky: &NightSky, position: IcrsPosition) -> Result<Among, NightError> {
        let (seen, declination) = sky.steps[self.reference]
            .observer
            .observe_with_declination(position)?;
        let (sin_declination, cos_declination) = declination.to_radians().sin_cos();
        let (sin_around, cos_around) =
            (seen.hour_angle_degrees.to_radians() - self.turned).sin_cos();
        Ok([
            cos_declination * cos_around,
            cos_declination * sin_around,
            sin_declination,
        ])
    }

    /// The track of a place among the stars on this night. The place need
    /// not be of unit length: one drawn between two others is a little
    /// short.
    fn track_among(&self, among: Among) -> Track {
        let length = (among[0] * among[0] + among[1] * among[1] + among[2] * among[2]).sqrt();
        let [x, y, z] = among.map(|part| part / length);
        let cos_declination = x.hypot(y);
        // At a pole the hour angle means nothing and multiplies nothing.
        let (cos_around, sin_around) = if cos_declination > 1e-12 {
            (x / cos_declination, y / cos_declination)
        } else {
            (1.0, 0.0)
        };
        Track {
            sin_declination: z,
            cos_declination,
            sin_hour: sin_around * self.cos_turned + cos_around * self.sin_turned,
            cos_hour: cos_around * self.cos_turned - sin_around * self.sin_turned,
        }
    }

    /// The cosine and sine of the place's hour angle at sample `index`,
    /// and the sine of its altitude there, as worked out.
    fn turned(&self, index: usize, track: Track) -> (f64, f64, f64) {
        let (cos_turn, sin_turn) = self.turns[index];
        let cos_hour = track.cos_hour * cos_turn - track.sin_hour * sin_turn;
        let sin_hour = track.sin_hour * cos_turn + track.cos_hour * sin_turn;
        let sin_altitude = track.sin_declination * self.sin_latitude
            + track.cos_declination * cos_hour * self.cos_latitude;
        (cos_hour, sin_hour, sin_altitude)
    }

    /// The place's azimuth in degrees, from north through east, for the
    /// hour angle [`Quick::turned`] worked out.
    fn azimuth(&self, track: Track, cos_hour: f64, sin_hour: f64) -> f64 {
        let north = track.sin_declination * self.cos_latitude
            - track.cos_declination * cos_hour * self.sin_latitude;
        let east = -track.cos_declination * sin_hour;
        east.atan2(north).to_degrees().rem_euclid(360.0)
    }

    /// The place's azimuth and altitude at sample `index`, in degrees, as
    /// worked out.
    #[cfg(test)]
    fn direction(&self, index: usize, track: Track) -> (f64, f64) {
        let (cos_hour, sin_hour, sin_altitude) = self.turned(index, track);
        (
            self.azimuth(track, cos_hour, sin_hour),
            sin_altitude.clamp(-1.0, 1.0).asin().to_degrees(),
        )
    }

    /// Whether the place on `track` is in view at sample `index`, when its
    /// worked-out direction is clear of every change of answer by
    /// [`SURE_DEGREES`]; `None` asks for the exact observation.
    fn answer(&self, test: &AltitudeTest<'_>, index: usize, track: Track) -> Option<bool> {
        let (cos_hour, sin_hour, sin_altitude) = self.turned(index, track);
        // NaN compares false throughout and falls to the exact path.
        if sin_altitude < self.out_below || sin_altitude > self.out_above {
            return Some(false);
        }
        if sin_altitude > self.in_above && sin_altitude < self.in_below {
            return Some(true);
        }
        let by_degree = self.by_degree.as_ref()?;
        if !sin_altitude.is_finite() {
            return None;
        }
        // Between the curve's lowest and highest: the azimuth decides.
        let azimuth = self.azimuth(track, cos_hour, sin_hour);
        // Mostly the curve within a degree settles it. Steeply up, or on
        // the curve's own edge, ask the curve itself.
        if sin_altitude < STEEP_SINE
            && let Some((under, over)) = by_degree.get(azimuth as usize)
        {
            if sin_altitude < *under {
                return Some(false);
            }
            if sin_altitude > *over && sin_altitude < self.in_below {
                return Some(true);
            }
        }
        let altitude = sin_altitude.clamp(-1.0, 1.0).asin().to_degrees();
        test.sure(azimuth, altitude, SURE_DEGREES)
    }
}

/// The longest unbroken time `position` is in view in the dark on the
/// night of `sky`: Sun below `dark_below_degrees`, and inside `horizon` and
/// `limits`. For many positions on one night, build a [`DarkRuns`] once.
pub fn longest_dark_run_ms(
    sky: &NightSky,
    horizon: &Horizon,
    limits: AltitudeLimits,
    dark_below_degrees: f64,
    position: IcrsPosition,
) -> Result<u64, NightError> {
    DarkRuns::new(sky, horizon, limits, dark_below_degrees)?.longest_ms(position)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::night::{night_summary_under, NightRequest, NightTarget};
    use crate::visibility::{altitude_allowed, HorizonPoint, Site};
    use crate::windows::MeridianExclusion;

    const STEP_MS: u64 = 300_000;
    const DARK: f64 = -12.0;
    /// Local noon in Los Angeles on 2026-09-24, as the night tests use.
    const START_MS: u64 = 1_790_294_400_000;

    fn site() -> Site {
        Site {
            latitude_degrees: 34.2,
            longitude_degrees: -118.3,
            elevation_meters: 400.0,
        }
    }

    fn limits(minimum: f64) -> AltitudeLimits {
        AltitudeLimits {
            rig_minimum_degrees: minimum,
            project_minimum_degrees: -90.0,
            horizon_offset_degrees: 0.0,
            rig_maximum_degrees: 90.0,
            project_maximum_degrees: 90.0,
        }
    }

    fn request(horizon: Horizon, limits: AltitudeLimits, position: IcrsPosition) -> NightRequest {
        NightRequest {
            site: site(),
            horizon,
            limits,
            targets: vec![NightTarget {
                id: "there".into(),
                position,
                moon: vec![],
            }],
            start_ms: START_MS,
            nights: 1,
            step_ms: STEP_MS,
            dark_below_degrees: DARK,
            meridian_exclusion: MeridianExclusion {
                before_ms: 0,
                after_ms: 0,
            },
            meridian_window_ms: 0,
            available: None,
        }
    }

    fn sky_of(request: &NightRequest) -> NightSky {
        NightSky::new(request.site, request.noon_ms(0).unwrap(), request.step_ms).unwrap()
    }

    fn at(ra: f64, dec: f64) -> IcrsPosition {
        IcrsPosition {
            ra_degrees: ra,
            dec_degrees: dec,
        }
    }

    /// A wall `height` degrees tall between two azimuths, flat elsewhere.
    fn wall(from: f64, to: f64, height: f64) -> Horizon {
        let point = |azimuth_degrees, altitude_degrees| HorizonPoint {
            azimuth_degrees,
            altitude_degrees,
        };
        Horizon::Custom {
            points: vec![
                point(0.0, 0.0),
                point(from - 0.5, 0.0),
                point(from, height),
                point(to, height),
                point(to + 0.5, 0.0),
                point(360.0, 0.0),
            ],
        }
    }

    #[test]
    fn under_a_flat_horizon_the_longest_run_is_the_hours_up() {
        // One rise and one set at most: the run is the whole sum.
        for position in [at(38.2, 61.45), at(300.0, -20.0), at(120.0, 10.0)] {
            let request = request(Horizon::FixedMinimum {}, limits(25.0), position);
            let sky = sky_of(&request);
            let night = night_summary_under(&request, 0, &sky).unwrap();
            let run = longest_dark_run_ms(&sky, &request.horizon, request.limits, DARK, position)
                .unwrap();
            let hours = run as f64 / 3_600_000.0;
            assert!(
                (hours - night.targets[0].hours_up).abs() < 1e-9,
                "{position:?}: {hours} against {}",
                night.targets[0].hours_up
            );
        }
        // The far south never clears 25° from 34° north.
        let request = request(Horizon::FixedMinimum {}, limits(25.0), at(38.2, -75.0));
        assert_eq!(
            longest_dark_run_ms(
                &sky_of(&request),
                &request.horizon,
                request.limits,
                DARK,
                at(38.2, -75.0)
            ),
            Ok(0)
        );
    }

    #[test]
    fn an_obstruction_splits_a_run_the_sum_would_pass() {
        // On the equator and due south at local midnight: up all night,
        // crossing the meridian in the middle of it.
        let position = at(0.0, 0.0);
        let open = request(Horizon::FixedMinimum {}, limits(20.0), position);
        let sky = sky_of(&open);
        // Each dark sample's azimuth while the place is above 20°.
        let dusk_dawn = sky.darkness(DARK).unwrap();
        let up: Vec<(u64, f64)> = NightSky::instants(sky.noon_ms(), STEP_MS)
            .unwrap()
            .into_iter()
            .filter(|t| (dusk_dawn.0..dusk_dawn.1).contains(t))
            .filter_map(|t| {
                let seen = sky.observe_at(t, position)?.ok()?;
                altitude_allowed(
                    &open.horizon,
                    open.limits,
                    seen.azimuth_degrees,
                    seen.altitude_degrees,
                )
                .unwrap()
                .then_some((t, seen.azimuth_degrees))
            })
            .collect();
        let whole = longest_dark_run_ms(&sky, &open.horizon, open.limits, DARK, position).unwrap();
        assert_eq!(whole, up.len() as u64 * STEP_MS);
        assert!(whole > 6 * 3_600_000, "{whole}");
        // A wall across the azimuths of the middle three samples.
        let middle = &up[up.len() / 2 - 1..=up.len() / 2 + 1];
        let low = middle.iter().map(|(_, az)| *az).fold(f64::MAX, f64::min);
        let high = middle.iter().map(|(_, az)| *az).fold(f64::MIN, f64::max);
        let walled = request(wall(low - 0.1, high + 0.1, 85.0), limits(20.0), position);
        let run =
            longest_dark_run_ms(&sky, &walled.horizon, walled.limits, DARK, position).unwrap();
        let night = night_summary_under(&walled, 0, &sky).unwrap();
        let summed_ms = (night.targets[0].hours_up * 3_600_000.0).round() as u64;
        // The sum loses only the samples behind the wall; the run is cut
        // to the longer side of it.
        assert_eq!(summed_ms, whole - 3 * STEP_MS);
        assert!(run < summed_ms, "{run} against {summed_ms}");
        assert!(run >= (whole - 3 * STEP_MS) / 2, "{run}");
        assert!(run <= whole / 2 + STEP_MS, "{run}");
        // A minimum between the two fails on the run and passes on the sum.
        let minimum = run + STEP_MS;
        assert!(minimum <= summed_ms);
    }

    #[test]
    fn a_night_with_no_darkness_has_no_run() {
        // Tromsø at midsummer: the Sun never reaches −12°.
        let site = Site {
            latitude_degrees: 69.6,
            longitude_degrees: 18.9,
            elevation_meters: 10.0,
        };
        let noon = crate::night::night_noon(1_781_740_800_000, site.longitude_degrees).unwrap();
        let sky = NightSky::new(site, noon, STEP_MS).unwrap();
        assert_eq!(sky.darkness(DARK), None);
        let horizon = Horizon::FixedMinimum {};
        let runs = DarkRuns::new(&sky, &horizon, limits(0.0), DARK).unwrap();
        // The pole is 69.6° up all day and still counts for nothing.
        assert_eq!(runs.longest_ms(at(0.0, 90.0)), Ok(0));
        assert_eq!(runs.longest_ms(at(200.0, 60.0)), Ok(0));
    }

    #[test]
    fn circumpolar_sky_runs_the_whole_night_and_the_wrap_is_seamless() {
        let horizon = Horizon::FixedMinimum {};
        let request = request(horizon.clone(), limits(20.0), at(0.0, 89.0));
        let sky = sky_of(&request);
        let (dusk, dawn) = sky.darkness(DARK).unwrap();
        let runs = DarkRuns::new(&sky, &horizon, request.limits, DARK).unwrap();
        // Within 14° of the pole nothing dips under 20° at 34.2° north.
        for ra in [0.0, 90.0, 180.0, 359.999] {
            assert_eq!(runs.longest_ms(at(ra, 80.0)), Ok(dawn - dusk), "{ra}");
        }
        assert_eq!(runs.longest_ms(at(0.0, 90.0)), Ok(dawn - dusk));
        // Asking whether a run is long enough agrees with measuring it.
        let steps = ((dawn - dusk) / STEP_MS) as u32;
        assert_eq!(runs.reaches(at(0.0, 80.0), steps), Ok(true));
        assert_eq!(runs.reaches(at(0.0, 80.0), steps + 1), Ok(false));
        assert_eq!(runs.reaches(at(0.0, -80.0), 1), Ok(false));
        assert_eq!(runs.reaches(at(0.0, -80.0), 0), Ok(true));
        // Either side of 0h is the same sky a hair apart.
        for dec in [-30.0, 0.0, 40.0] {
            let west = runs.longest_ms(at(359.999, dec)).unwrap();
            let east = runs.longest_ms(at(0.001, dec)).unwrap();
            assert!(west.abs_diff(east) <= STEP_MS, "{dec}: {west} {east}");
        }
        // 360° itself is not a right ascension.
        assert!(runs.longest_ms(at(360.0, 0.0)).is_err());
    }

    #[test]
    fn asking_whether_a_run_is_long_enough_agrees_with_measuring_it() {
        // Two walls, so a night can hold several runs of different
        // lengths, and a maximum altitude, which cuts runs at the top.
        let point = |azimuth_degrees, altitude_degrees| HorizonPoint {
            azimuth_degrees,
            altitude_degrees,
        };
        let horizon = Horizon::Custom {
            points: vec![
                point(0.0, 5.0),
                point(100.0, 5.0),
                point(101.0, 60.0),
                point(125.0, 60.0),
                point(126.0, 15.0),
                point(200.0, 15.0),
                point(201.0, 75.0),
                point(230.0, 75.0),
                point(231.0, 5.0),
                point(360.0, 5.0),
            ],
        };
        let mut limits = limits(10.0);
        limits.rig_maximum_degrees = 80.0;
        let sky = sky_of(&request(horizon.clone(), limits, at(0.0, 0.0)));
        let runs = DarkRuns::new(&sky, &horizon, limits, DARK).unwrap();
        let mut lengths = std::collections::BTreeSet::new();
        for ra in (0..360).step_by(15) {
            for dec in (-60..=90).step_by(10) {
                let position = at(f64::from(ra), f64::from(dec));
                let longest = runs.longest_steps(position).unwrap();
                lengths.insert(longest);
                for steps in [1, 2, 3, 5, 6, 7, 12, 24, 37, 60, 100, 144, 288] {
                    assert_eq!(
                        runs.reaches(position, steps).unwrap(),
                        longest >= steps,
                        "{position:?} for {steps}: longest {longest}"
                    );
                }
                // Exactly at its own length, and one past it.
                assert!(runs.reaches(position, longest).unwrap(), "{position:?}");
                assert!(
                    !runs.reaches(position, longest + 1).unwrap(),
                    "{position:?}"
                );
            }
        }
        // The walls and the night between them gave runs of many lengths.
        assert!(lengths.len() > 20, "{lengths:?}");
    }

    /// Sites from pole to pole, with a longitude each.
    const SITES: [(f64, f64); 8] = [
        (-89.5, 0.0),
        (-75.0, 123.0),
        (-33.9, 18.5),
        (0.5, -78.5),
        (40.0, -105.3),
        (52.0, 0.0),
        (69.6, 18.9),
        (89.5, -170.0),
    ];
    /// Instants across the years the astronomy times: both sides of the
    /// leap seconds at the end of June 1972 and of 2016, the solstices and
    /// equinoxes, and the last years.
    const INSTANTS: [u64; 8] = [
        78_796_800_000,    // 1972-07-01 00:00 UTC, as a leap second ends
        1_483_228_800_000, // 2017-01-01 00:00 UTC, as a leap second ends
        1_781_740_800_000, // 2026-06-18
        1_790_294_400_000, // 2026-09-25
        1_797_638_400_000, // 2026-12-19
        1_805_500_800_000, // 2027-03-20
        2_845_000_000_000, // 2060-02-26
        4_070_000_000_000, // 2098-12-21
    ];

    fn sky_at(latitude: f64, longitude: f64, instant_ms: u64) -> NightSky {
        let site = Site {
            latitude_degrees: latitude,
            longitude_degrees: longitude,
            elevation_meters: 100.0,
        };
        let noon = crate::night::night_noon(instant_ms, longitude).unwrap();
        NightSky::new(site, noon, STEP_MS).unwrap()
    }

    /// A horizon as N.I.N.A. writes one: jagged every 7° and with ends
    /// that differ at north.
    fn jagged() -> Horizon {
        let mut points = vec![HorizonPoint {
            azimuth_degrees: 0.0,
            altitude_degrees: 12.0,
        }];
        for azimuth in (7..360).step_by(7) {
            points.push(HorizonPoint {
                azimuth_degrees: f64::from(azimuth),
                altitude_degrees: 8.0 + f64::from(17 * azimuth % 31),
            });
        }
        points.push(HorizonPoint {
            azimuth_degrees: 360.0,
            altitude_degrees: 25.0,
        });
        Horizon::Custom { points }
    }

    fn separation_degrees(a: (f64, f64), b: (f64, f64)) -> f64 {
        let unit = |(azimuth, altitude): (f64, f64)| {
            let (azimuth, altitude) = (azimuth.to_radians(), altitude.to_radians());
            [
                altitude.cos() * azimuth.cos(),
                altitude.cos() * azimuth.sin(),
                altitude.sin(),
            ]
        };
        let (a, b) = (unit(a), unit(b));
        let cross = [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ];
        let sine = (cross[0] * cross[0] + cross[1] * cross[1] + cross[2] * cross[2]).sqrt();
        let cosine = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        sine.atan2(cosine).to_degrees()
    }

    #[test]
    fn the_worked_out_direction_is_within_a_tenth_of_the_margin() {
        // The shortcut is safe while the direction it works out is within
        // SURE_DEGREES of the observed one. Hold it to a tenth of that, at
        // every dark sample, from pole to pole and from 1972 to 2098.
        let horizon = Horizon::FixedMinimum {};
        let mut worst: f64 = 0.0;
        let mut compared = 0;
        for (latitude, longitude) in SITES {
            for instant in INSTANTS {
                let sky = sky_at(latitude, longitude, instant);
                let runs = DarkRuns::new(&sky, &horizon, limits(20.0), DARK).unwrap();
                let Some(quick) = &runs.quick else {
                    continue;
                };
                for ra in (0..360).step_by(30) {
                    for dec in [-90.0, -89.9, -60.0, -30.0, 0.0, 30.0, 60.0, 89.9, 90.0] {
                        let position = at(f64::from(ra), dec);
                        let track = quick.track(&sky, position).unwrap();
                        for index in runs.blocks.iter().flat_map(|block| block.clone()) {
                            let seen = sky.steps[index].observer.observe(position).unwrap();
                            let exact = (seen.azimuth_degrees, seen.altitude_degrees);
                            let apart = separation_degrees(quick.direction(index, track), exact);
                            assert!(
                                apart < SURE_DEGREES / 10.0,
                                "{apart}° at {latitude},{longitude} {instant} {position:?} sample {index}"
                            );
                            worst = worst.max(apart);
                            compared += 1;
                        }
                    }
                }
            }
        }
        // Enough nights were dark to mean something, and the model's error
        // is what the module says it is: about an arcsecond.
        assert!(compared > 400_000, "{compared}");
        assert!(worst < 2.0 / 3600.0, "{} arcseconds", worst * 3600.0);
        eprintln!(
            "worst worked-out direction: {:.3} arcseconds over {compared} samples",
            worst * 3600.0
        );
    }

    #[test]
    fn a_place_drawn_between_two_nights_is_within_a_tenth_of_the_margin() {
        // Seventeen nights in a row: observed on the first, ninth and last,
        // and drawn between. An hourly sky is enough to place a direction.
        const HOURLY: u64 = 3_600_000;
        let horizon = Horizon::FixedMinimum {};
        let mut worst: f64 = 0.0;
        let (mut compared, mut drawn) = (0, 0);
        for (latitude, longitude) in SITES {
            for instant in [
                INSTANTS[0],
                INSTANTS[2],
                INSTANTS[3],
                INSTANTS[5],
                INSTANTS[7],
            ] {
                let site = Site {
                    latitude_degrees: latitude,
                    longitude_degrees: longitude,
                    elevation_meters: 100.0,
                };
                let first = crate::night::night_noon(instant, longitude).unwrap();
                let Ok(skies) = (0..17)
                    .map(|night| NightSky::new(site, first + night * 86_400_000, HOURLY))
                    .collect::<Result<Vec<_>, _>>()
                else {
                    // Past the last year the astronomy times.
                    continue;
                };
                let nights: Vec<DarkRuns<'_>> = skies
                    .iter()
                    .map(|sky| DarkRuns::new(sky, &horizon, limits(20.0), DARK).unwrap())
                    .collect();
                for ra in (0..360).step_by(40) {
                    for dec in [-90.0, -89.9, -55.0, -20.0, 0.0, 35.0, 70.0, 89.9, 90.0] {
                        let position = at(f64::from(ra) + 3.0, dec);
                        let placed = place_over(&nights, position).unwrap();
                        for (night, runs) in nights.iter().enumerate() {
                            let Some(quick) = &runs.quick else {
                                assert!(placed.0[night].is_none());
                                continue;
                            };
                            // Every dark night within reach of another is placed.
                            let Some(track) = placed.0[night] else {
                                continue;
                            };
                            drawn += 1;
                            for index in runs.blocks.iter().flat_map(|block| block.clone()) {
                                let seen = skies[night].steps[index]
                                    .observer
                                    .observe(position)
                                    .unwrap();
                                let apart = separation_degrees(
                                    quick.direction(index, track),
                                    (seen.azimuth_degrees, seen.altitude_degrees),
                                );
                                assert!(
                                    apart < SURE_DEGREES / 10.0,
                                    "{apart}° at {latitude},{longitude} {instant} night {night} {position:?}"
                                );
                                worst = worst.max(apart);
                                compared += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(drawn > 20_000 && compared > 200_000, "{drawn} {compared}");
        eprintln!(
            "worst direction drawn between nights: {:.3} arcseconds over {compared} samples",
            worst * 3600.0
        );
    }

    #[test]
    fn nights_placed_together_give_the_plain_answers() {
        let walled = wall(120.0, 150.0, 65.0);
        let jagged = jagged();
        for ((latitude, longitude), instant, horizon, nights) in [
            (SITES[4], INSTANTS[4], &walled, 12_u64),
            (SITES[2], INSTANTS[2], &jagged, 10),
            // Tromsø as the dark comes back: the first nights have none.
            (SITES[6], 1_787_270_400_000, &walled, 14),
        ] {
            let site = Site {
                latitude_degrees: latitude,
                longitude_degrees: longitude,
                elevation_meters: 100.0,
            };
            let first = crate::night::night_noon(instant, longitude).unwrap();
            let skies: Vec<NightSky> = (0..nights)
                .map(|night| NightSky::new(site, first + night * 86_400_000, STEP_MS).unwrap())
                .collect();
            let short: Vec<DarkRuns<'_>> = skies
                .iter()
                .map(|sky| DarkRuns::new(sky, horizon, limits(15.0), DARK).unwrap())
                .collect();
            let plain: Vec<DarkRuns<'_>> = skies
                .iter()
                .map(|sky| DarkRuns::plain(sky, horizon, limits(15.0), DARK).unwrap())
                .collect();
            let mut answers = std::collections::BTreeSet::new();
            for ra in (0..360).step_by(20) {
                for dec in (-9..=9).map(|step| f64::from(step) * 10.0) {
                    let position = at(f64::from(ra) + 0.61, dec);
                    let placed = place_over(&short, position).unwrap();
                    for night in 0..skies.len() {
                        let longest = plain[night].longest_steps(position).unwrap();
                        for steps in [1, 3, 6, 36] {
                            let answer = short[night]
                                .reaches_placed(position, steps, &placed, night)
                                .unwrap();
                            assert_eq!(
                                answer,
                                longest >= steps,
                                "{latitude},{longitude} night {night} {position:?} for {steps}"
                            );
                            answers.insert(answer);
                        }
                    }
                }
            }
            assert_eq!(answers.len(), 2);
            // Observed on a few nights, not on each: under a third of the
            // places' nights were observed to place them.
            use std::sync::atomic::Ordering::Relaxed;
            let dark = short.iter().filter(|runs| runs.quick.is_some()).count();
            assert!(dark >= 3, "{dark}");
            let observed: usize = short.iter().map(|runs| runs.counted.1.load(Relaxed)).sum();
            let answered: usize = short.iter().map(|runs| runs.counted.0.load(Relaxed)).sum();
            assert!(answered > 50 * observed, "{answered} {observed}");
        }
        // Nights out of order, or of two sites, are each observed: the
        // answers stand, and nothing is placed.
        let sky_a = sky_at(40.0, -105.3, INSTANTS[4]);
        let sky_b = sky_at(-33.9, 18.5, INSTANTS[4]);
        let flat = Horizon::FixedMinimum {};
        let mixed = [
            DarkRuns::new(&sky_a, &flat, limits(20.0), DARK).unwrap(),
            DarkRuns::new(&sky_b, &flat, limits(20.0), DARK).unwrap(),
        ];
        let position = at(80.0, 10.0);
        let placed = place_over(&mixed, position).unwrap();
        assert!(placed.0.iter().all(|track| track.is_none()));
        for (night, runs) in mixed.iter().enumerate() {
            assert_eq!(
                runs.reaches_placed(position, 6, &placed, night).unwrap(),
                runs.reaches(position, 6).unwrap()
            );
        }
        // A night past the end of what was placed places itself.
        assert_eq!(
            mixed[0].reaches_placed(position, 6, &placed, 9).unwrap(),
            mixed[0].reaches(position, 6).unwrap()
        );
    }

    #[test]
    fn the_shortcut_gives_the_plain_answers() {
        // Every run length, at every place on a grid of the sky, under
        // flat, walled, narrow and jagged horizons and several limits.
        let narrow = wall(180.0, 181.0, 70.0);
        let walled = wall(120.0, 150.0, 65.0);
        let jagged = jagged();
        let flat = Horizon::FixedMinimum {};
        let mut capped = limits(-90.0);
        capped.rig_maximum_degrees = 70.0;
        capped.project_minimum_degrees = 5.0;
        capped.horizon_offset_degrees = 3.0;
        let cases: [(&Horizon, AltitudeLimits); 6] = [
            (&flat, limits(20.0)),
            (&flat, capped),
            (&walled, limits(20.0)),
            (&narrow, limits(0.0)),
            (&jagged, limits(10.0)),
            (&jagged, capped),
        ];
        let (mut quick, mut exact, mut lengths) = (0, 0, std::collections::BTreeSet::new());
        for (latitude, longitude) in [SITES[2], SITES[3], SITES[4], SITES[6], SITES[7]] {
            for instant in [INSTANTS[0], INSTANTS[2], INSTANTS[4], INSTANTS[7]] {
                let sky = sky_at(latitude, longitude, instant);
                for (horizon, limits) in cases {
                    let short = DarkRuns::new(&sky, horizon, limits, DARK).unwrap();
                    let plain = DarkRuns::plain(&sky, horizon, limits, DARK).unwrap();
                    assert!(plain.quick.is_none());
                    for ra in (0..360).step_by(24) {
                        for dec in (-12..=12).map(|step| f64::from(step) * 7.5) {
                            let position = at(f64::from(ra) + 0.37, dec);
                            let longest = plain.longest_steps(position).unwrap();
                            assert_eq!(
                                short.longest_steps(position).unwrap(),
                                longest,
                                "{latitude},{longitude} {instant} {position:?}"
                            );
                            for steps in [1, 2, 6, 7, 24, 48, 120] {
                                assert_eq!(
                                    short.reaches(position, steps).unwrap(),
                                    longest >= steps,
                                    "{latitude},{longitude} {instant} {position:?} for {steps}"
                                );
                            }
                            lengths.insert(longest);
                        }
                    }
                    use std::sync::atomic::Ordering::Relaxed;
                    quick += short.counted.0.load(Relaxed);
                    exact += short.counted.1.load(Relaxed);
                    assert_eq!(plain.counted.0.load(Relaxed), 0);
                }
            }
        }
        assert!(lengths.len() > 60, "{lengths:?}");
        // The shortcut answered nearly every sample, and the samples it
        // would not answer were observed.
        assert!(exact > 0);
        assert!(quick > 50 * exact, "{quick} answered, {exact} observed");
        eprintln!("the shortcut answered {quick} samples; {exact} were observed");
    }

    #[test]
    fn a_sample_on_a_limit_is_observed_not_guessed() {
        // A minimum set to a place's own altitude at a sample, and a hair
        // either side of it: the rule is strict, and the shortcut must not
        // round it.
        let position = at(0.0, 0.0);
        let sky = sky_of(&request(Horizon::FixedMinimum {}, limits(20.0), position));
        let (dusk, _) = sky.darkness(DARK).unwrap();
        let horizon = Horizon::FixedMinimum {};
        for sample in [6_u64, 30, 60] {
            let at_ms = dusk + sample * STEP_MS;
            let altitude = sky
                .observe_at(at_ms, position)
                .unwrap()
                .unwrap()
                .altitude_degrees;
            if !(1.0..89.0).contains(&altitude) {
                continue;
            }
            let mut answers = Vec::new();
            for minimum in [altitude - 1e-9, altitude, altitude + 1e-9] {
                let short = DarkRuns::new(&sky, &horizon, limits(minimum), DARK).unwrap();
                let plain = DarkRuns::plain(&sky, &horizon, limits(minimum), DARK).unwrap();
                assert_eq!(
                    short.longest_steps(position).unwrap(),
                    plain.longest_steps(position).unwrap(),
                    "minimum {minimum}"
                );
                assert!(short.counted.1.load(std::sync::atomic::Ordering::Relaxed) > 0);
                answers.push(plain.longest_steps(position).unwrap());
            }
            // The sample itself is in view only under the lower minimum,
            // so the hair changes the run.
            assert_ne!(answers[0], answers[1], "sample {sample}");
            assert_eq!(answers[1], answers[2]);
        }
        // A curve that passes exactly through a place's direction.
        let at_ms = dusk + 30 * STEP_MS;
        let seen = sky.observe_at(at_ms, position).unwrap().unwrap();
        let through = Horizon::Custom {
            points: vec![
                HorizonPoint {
                    azimuth_degrees: 0.0,
                    altitude_degrees: seen.altitude_degrees,
                },
                HorizonPoint {
                    azimuth_degrees: 360.0,
                    altitude_degrees: seen.altitude_degrees,
                },
            ],
        };
        let short = DarkRuns::new(&sky, &through, limits(0.0), DARK).unwrap();
        let plain = DarkRuns::plain(&sky, &through, limits(0.0), DARK).unwrap();
        assert_eq!(
            short.longest_steps(position).unwrap(),
            plain.longest_steps(position).unwrap()
        );
    }

    #[test]
    fn bad_limits_and_horizons_are_refused_once() {
        let request = request(Horizon::FixedMinimum {}, limits(20.0), at(0.0, 0.0));
        let sky = sky_of(&request);
        let mut upside_down = limits(20.0);
        upside_down.rig_maximum_degrees = 10.0;
        assert!(DarkRuns::new(&sky, &request.horizon, upside_down, DARK).is_err());
        let broken = Horizon::Custom { points: vec![] };
        assert!(DarkRuns::new(&sky, &broken, limits(20.0), DARK).is_err());
        assert!(DarkRuns::new(&sky, &request.horizon, limits(20.0), f64::NAN).is_err());
    }
}
