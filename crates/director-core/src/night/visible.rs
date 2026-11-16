//! How long a place in the sky stays in view on one night: its longest
//! unbroken run of dark samples inside the horizon and the altitude limits.
//! A night's hours up are a sum, which a tree in the way splits without
//! shortening; a rig that needs half an hour on a target needs it in one
//! piece.

use super::{NightError, NightSky};
use crate::visibility::{AltitudeLimits, AltitudeTest, Horizon, IcrsPosition};

/// One night's sky under one horizon and one set of limits, ready to
/// measure many positions: the dark samples are found once, and the
/// horizon and limits are validated once.
pub struct DarkRuns<'a> {
    sky: &'a NightSky,
    test: AltitudeTest<'a>,
    /// The sky's dark samples, as runs of consecutive samples: one run on
    /// an ordinary night, none when it never gets dark.
    blocks: Vec<std::ops::Range<usize>>,
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
        })
    }

    /// The sample step, in milliseconds.
    pub fn step_ms(&self) -> u64 {
        self.sky.step_ms
    }

    /// Whether `position` is in view at sample `index`.
    fn in_view(&self, index: usize, position: IcrsPosition) -> Result<bool, NightError> {
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
        for block in &self.blocks {
            let mut run = 0;
            for index in block.clone() {
                if self.in_view(index, position)? {
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
        let needed = steps as usize;
        if needed == 0 {
            return Ok(true);
        }
        for block in &self.blocks {
            let mut probe = block.start + needed - 1;
            while probe < block.end {
                if self.in_view(probe, position)? {
                    let mut run = 1;
                    let mut before = probe;
                    while run < needed && before > block.start {
                        before -= 1;
                        if !self.in_view(before, position)? {
                            break;
                        }
                        run += 1;
                    }
                    let mut after = probe + 1;
                    while run < needed && after < block.end {
                        if !self.in_view(after, position)? {
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
