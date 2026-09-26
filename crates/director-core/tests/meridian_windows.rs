use psf_guard_director_core::{visibility::*, windows::*};

const START: u64 = 1_790_409_600_000;
const MINUTE: u64 = 60_000;

fn inputs() -> (IcrsPosition, Site, EarthOrientation) {
    (
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
            valid_from_ms: START - 86_400_000,
            valid_until_ms: START + 86_400_001,
        },
    )
}

fn at_hour_angle(at: u64, angle: f64) -> IcrsPosition {
    let (mut target, site, eop) = inputs();
    for _ in 0..8 {
        let error = observe(target, site, eop, at).unwrap().hour_angle_degrees - angle;
        target.ra_degrees = (target.ra_degrees + error).rem_euclid(360.0);
    }
    let actual = observe(target, site, eop, at).unwrap().hour_angle_degrees;
    assert!(((actual - angle + 180.0).rem_euclid(360.0) - 180.0).abs() < 1e-8);
    target
}

fn interval(start: u64, end: u64) -> Interval {
    Interval {
        start_ms: start,
        end_ms: end,
    }
}

fn overlaps(a: Interval, b: Interval) -> bool {
    a.start_ms < b.end_ms && b.start_ms < a.end_ms
}

fn contains(windows: &[Interval], at: u64) -> bool {
    windows.iter().any(|w| w.start_ms <= at && at < w.end_ms)
}

#[test]
fn independent_margins_expand_the_entire_possible_crossing_band() {
    let (_, site, eop) = inputs();
    let transit = START + 30 * MINUTE;
    let target = at_hour_angle(transit, 0.0);
    let assignment = interval(START, START + 60 * MINUTE);
    for (before_ms, after_ms) in [(0, MINUTE), (MINUTE, 0), (3 * MINUTE, 7 * MINUTE)] {
        let result = meridian_windows(
            target,
            site,
            eop,
            assignment,
            MeridianExclusion {
                before_ms,
                after_ms,
            },
        )
        .unwrap();
        assert_eq!(result.assignment, assignment);
        assert_eq!(
            result.searched,
            Some(interval(START - after_ms, assignment.end_ms + before_ms))
        );
        assert_eq!(result.possible_transits.len(), 1);
        let band = result.possible_transits[0];
        assert!(band.start_ms <= transit && transit <= band.end_ms);
        assert!(band.end_ms - band.start_ms < 10_000);
        assert_eq!(
            result.windows,
            vec![
                interval(START, band.start_ms - before_ms),
                interval(band.end_ms + after_ms, assignment.end_ms),
            ]
        );
        let excluded = interval(transit - before_ms, transit + after_ms);
        assert!(result.windows.iter().all(|w| !overlaps(*w, excluded)));
        let json = serde_json::to_string(&result).unwrap();
        assert_eq!(
            serde_json::from_str::<MeridianWindows>(&json).unwrap(),
            result
        );
    }
}

#[test]
fn transits_outside_assignment_still_remove_overlapping_margins() {
    let (_, site, eop) = inputs();
    let assignment = interval(START, START + 10 * MINUTE);
    for transit in [START - MINUTE, assignment.end_ms + MINUTE] {
        let result = meridian_windows(
            at_hour_angle(transit, 0.0),
            site,
            eop,
            assignment,
            MeridianExclusion {
                before_ms: 2 * MINUTE,
                after_ms: 3 * MINUTE,
            },
        )
        .unwrap();
        assert_eq!(result.possible_transits.len(), 1);
        assert_eq!(result.windows.len(), 1);
        assert!(!overlaps(
            result.windows[0],
            interval(transit - 2 * MINUTE, transit + 3 * MINUTE)
        ));
        if transit < START {
            assert!(result.windows[0].start_ms >= START + 2 * MINUTE);
            assert_eq!(result.windows[0].end_ms, assignment.end_ms);
        } else {
            assert_eq!(result.windows[0].start_ms, START);
            assert!(result.windows[0].end_ms <= assignment.end_ms - MINUTE);
        }
    }
}

#[test]
fn no_upper_crossing_includes_lower_culmination_wrap() {
    let (_, site, eop) = inputs();
    let assignment = interval(START, START + 10 * MINUTE);
    for angle in [-90.0, 90.0, 180.0] {
        let target = at_hour_angle(START + 5 * MINUTE, angle);
        let result = meridian_windows(
            target,
            site,
            eop,
            assignment,
            MeridianExclusion {
                before_ms: MINUTE,
                after_ms: MINUTE,
            },
        )
        .unwrap();
        assert_eq!(result.windows, vec![assignment]);
        assert!(result.possible_transits.is_empty());
        if angle == 180.0 {
            assert!(
                observe(target, site, eop, START)
                    .unwrap()
                    .hour_angle_degrees
                    > 170.0
            );
            assert!(
                observe(target, site, eop, assignment.end_ms)
                    .unwrap()
                    .hour_angle_degrees
                    < -170.0
            );
        }
    }
}

#[test]
fn a_near_full_day_can_contain_two_upper_crossings() {
    let (_, site, eop) = inputs();
    let target = at_hour_angle(START + MINUTE, 0.0);
    let assignment = interval(START, START + 1438 * MINUTE);
    let result = meridian_windows(
        target,
        site,
        eop,
        assignment,
        MeridianExclusion {
            before_ms: MINUTE,
            after_ms: MINUTE,
        },
    )
    .unwrap();
    assert_eq!(result.possible_transits.len(), 2);
    assert!(result.possible_transits[0].start_ms < START + MINUTE);
    assert!(result.possible_transits[1].start_ms > START + 1436 * MINUTE);
    // Both endpoint exclusions overlap the assignment. Only the middle survives.
    assert_eq!(
        result.windows,
        vec![interval(
            result.possible_transits[0].end_ms + MINUTE,
            result.possible_transits[1].start_ms - MINUTE,
        )]
    );
}

#[test]
fn expanded_search_clamps_at_unix_epoch_without_wrapping() {
    let (target, site, eop) = inputs();
    let result = meridian_windows(
        target,
        site,
        EarthOrientation {
            valid_from_ms: 0,
            valid_until_ms: 3 * MINUTE,
            ..eop
        },
        interval(0, MINUTE),
        MeridianExclusion {
            before_ms: MINUTE,
            after_ms: MINUTE,
        },
    )
    .unwrap();
    assert_eq!(result.searched, Some(interval(0, 2 * MINUTE)));
    assert!(result.windows.iter().all(|w| w.end_ms <= MINUTE));
}

#[test]
fn explicit_zero_policy_needs_no_geometry_but_still_requires_an_interval() {
    let (mut target, mut site, mut eop) = inputs();
    target.ra_degrees = f64::NAN;
    site.latitude_degrees = f64::NAN;
    eop.valid_until_ms = 0;
    let assignment = interval(0, u64::MAX);
    let disabled = MeridianExclusion {
        before_ms: 0,
        after_ms: 0,
    };
    assert_eq!(
        meridian_windows(target, site, eop, assignment, disabled).unwrap(),
        MeridianWindows {
            assignment,
            searched: None,
            windows: vec![assignment],
            possible_transits: vec![]
        }
    );
    assert_eq!(
        meridian_windows(target, site, eop, interval(1, 1), disabled),
        Err(VisibilityError::InvalidSpan)
    );
}

#[test]
fn expanded_search_requires_fresh_supported_geometry_including_finish() {
    let (target, site, eop) = inputs();
    let assignment = interval(START, START + MINUTE);
    let policy = MeridianExclusion {
        before_ms: MINUTE,
        after_ms: 2 * MINUTE,
    };
    for stale in [
        EarthOrientation {
            valid_from_ms: START - 2 * MINUTE + 1,
            ..eop
        },
        EarthOrientation {
            valid_until_ms: START + 2 * MINUTE,
            ..eop
        },
    ] {
        assert_eq!(
            meridian_windows(target, site, stale, assignment, policy),
            Err(VisibilityError::StaleEarthOrientation)
        );
    }
    assert_eq!(
        meridian_windows(
            target,
            site,
            eop,
            interval(START, START + 86_400_000),
            policy
        ),
        Err(VisibilityError::InvalidSpan)
    );
    assert_eq!(
        meridian_windows(
            target,
            site,
            eop,
            assignment,
            MeridianExclusion {
                before_ms: u64::MAX,
                after_ms: 0
            }
        ),
        Err(VisibilityError::MeridianTimeOverflow)
    );
    assert_eq!(
        meridian_windows(
            IcrsPosition {
                dec_degrees: 91.0,
                ..target
            },
            site,
            eop,
            assignment,
            policy
        ),
        Err(VisibilityError::InvalidPosition)
    );
    // The assignment itself ends before the leap; the required search does not.
    let leap = 1_483_228_800_000;
    assert_eq!(
        meridian_windows(
            target,
            site,
            EarthOrientation {
                valid_from_ms: leap - 4 * MINUTE,
                valid_until_ms: leap + 4 * MINUTE,
                ..eop
            },
            interval(leap - 2 * MINUTE, leap - MINUTE),
            MeridianExclusion {
                before_ms: 2 * MINUTE,
                after_ms: 0
            }
        ),
        Err(VisibilityError::EarthOrientationDiscontinuity)
    );
}

#[test]
fn dense_crossing_oracle_is_always_excluded_across_sites_and_declinations() {
    let (target, site, eop) = inputs();
    let assignment = interval(START, START + 23 * 60 * MINUTE);
    let policy = MeridianExclusion {
        before_ms: 2 * MINUTE,
        after_ms: 5 * MINUTE,
    };
    for latitude in [-70.0, 0.0, 70.0] {
        for declination in [-89.0, -85.0, -5.0, 85.0, 89.0] {
            let target = IcrsPosition {
                dec_degrees: declination,
                ..target
            };
            let site = Site {
                latitude_degrees: latitude,
                ..site
            };
            let result = meridian_windows(target, site, eop, assignment, policy).unwrap();
            let searched = result.searched.unwrap();
            let mut previous = searched.start_ms;
            let mut previous_ha = observe(target, site, eop, previous)
                .unwrap()
                .hour_angle_degrees;
            let mut crossings = 0;
            for time in (searched.start_ms + MINUTE..searched.end_ms)
                .step_by(MINUTE as usize)
                .chain([searched.end_ms])
            {
                let ha = observe(target, site, eop, time).unwrap().hour_angle_degrees;
                if previous_ha < 0.0 && ha >= 0.0 && ha - previous_ha < 180.0 {
                    let (mut low, mut high) = (previous, time);
                    while high - low > 1 {
                        let middle = low + (high - low) / 2;
                        if observe(target, site, eop, middle)
                            .unwrap()
                            .hour_angle_degrees
                            < 0.0
                        {
                            low = middle;
                        } else {
                            high = middle;
                        }
                    }
                    crossings += 1;
                    assert!(contains(&result.possible_transits, low));
                    assert!(contains(&result.possible_transits, high));
                    let exclusion = interval(low - policy.before_ms, high + policy.after_ms);
                    assert!(result.windows.iter().all(|w| !overlaps(*w, exclusion)));
                }
                previous = time;
                previous_ha = ha;
            }
            assert_eq!(crossings, 1);
            assert!(result
                .windows
                .windows(2)
                .all(|w| w[0].end_ms < w[1].start_ms));
            assert!(result
                .windows
                .iter()
                .all(|w| w.start_ms >= assignment.start_ms && w.end_ms <= assignment.end_ms));
        }
    }
}
