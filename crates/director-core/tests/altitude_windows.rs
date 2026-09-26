use psf_guard_director_core::{visibility::*, windows::*, *};

const START: u64 = 1_790_409_600_000;
fn inputs() -> (IcrsPosition, Site, EarthOrientation, AltitudeLimits) {
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
            valid_from_ms: START,
            valid_until_ms: START + 86_400_001,
        },
        AltitudeLimits {
            rig_minimum_degrees: -89.0,
            project_minimum_degrees: -89.0,
            horizon_offset_degrees: 0.0,
            rig_maximum_degrees: 89.0,
            project_maximum_degrees: 89.0,
        },
    )
}
fn interval(duration: u64) -> Interval {
    Interval {
        start_ms: START,
        end_ms: START + duration,
    }
}
fn spike() -> Horizon {
    let (target, site, eop, _) = inputs();
    let az = observe(target, site, eop, START + 21_777)
        .unwrap()
        .azimuth_degrees;
    Horizon::Custom {
        points: [
            (0.0, -89.0),
            (az.next_down(), -89.0),
            (az, 89.0),
            (az.next_up(), -89.0),
            (360.0, -89.0),
        ]
        .into_iter()
        .map(|(azimuth_degrees, altitude_degrees)| HorizonPoint {
            azimuth_degrees,
            altitude_degrees,
        })
        .collect(),
    }
}
fn contains(intervals: &[Interval], at: u64) -> bool {
    intervals.iter().any(|w| w.start_ms <= at && at < w.end_ms)
}

#[test]
fn clear_regions_merge_without_fragmenting_the_night() {
    let (target, site, eop, limits) = inputs();
    for duration in [1, 999, 1000, 3_600_000, 86_400_000] {
        let searched = interval(duration);
        assert_eq!(
            altitude_windows(
                target,
                site,
                eop,
                &Horizon::FixedMinimum {},
                limits,
                searched
            )
            .unwrap(),
            AltitudeWindows {
                searched,
                windows: vec![searched],
                unresolved: vec![]
            }
        );
    }
}

#[test]
fn all_blocked_is_distinct_from_unresolved_and_errors() {
    let (target, site, eop, mut limits) = inputs();
    limits.rig_minimum_degrees = 80.0;
    let blocked = altitude_windows(
        target,
        site,
        eop,
        &Horizon::FixedMinimum {},
        limits,
        interval(86_400_000),
    )
    .unwrap();
    assert!(blocked.windows.is_empty());
    assert!(blocked.unresolved.is_empty());
    let mid = observe(target, site, eop, START + 500).unwrap();
    limits.rig_minimum_degrees = mid.altitude_degrees - 0.00001;
    let unresolved = altitude_windows(
        target,
        site,
        eop,
        &Horizon::FixedMinimum {},
        limits,
        interval(1000),
    )
    .unwrap();
    assert!(unresolved.windows.is_empty());
    assert_eq!(unresolved.unresolved, vec![interval(1000)]);
    limits.horizon_offset_degrees = -1.0;
    assert_eq!(
        altitude_windows(
            target,
            site,
            eop,
            &Horizon::FixedMinimum {},
            limits,
            interval(1000)
        ),
        Err(VisibilityError::InvalidAltitudeLimits)
    );
}

#[test]
fn unsampled_sub_millisecond_horizon_spike_splits_windows() {
    let (target, site, eop, limits) = inputs();
    let horizon = spike();
    let result = altitude_windows(target, site, eop, &horizon, limits, interval(60_000)).unwrap();
    assert_eq!(result.windows.len(), 2);
    assert!(contains(&result.windows, START));
    assert!(contains(&result.windows, START + 59_999));
    assert!(!contains(&result.windows, START + 21_777));
    assert!(contains(&result.unresolved, START + 21_777));
    for w in &result.windows {
        assert_eq!(
            check_altitude_span(target, site, eop, &horizon, limits, *w),
            Ok(AltitudeSpan::Clear)
        );
    }
    for w in &result.unresolved {
        assert!(!result
            .windows
            .iter()
            .any(|clear| clear.start_ms < w.end_ms && w.start_ms < clear.end_ms));
    }
}

#[test]
fn observed_windows_compose_with_meridian_exclusions_and_feed_core_selection() {
    let (target, site, eop, limits) = inputs();
    let result = altitude_windows(target, site, eop, &spike(), limits, interval(60_000)).unwrap();
    let policy = MeridianExclusion {
        before_ms: 2000,
        after_ms: 2000,
    };
    let transits = TransitCoverage {
        searched: Interval {
            start_ms: START - 2000,
            end_ms: START + 62_000,
        },
        transits_ms: vec![START + 40_000],
    };
    let composed =
        observing_windows(&result.windows, result.searched, policy, Some(&transits)).unwrap();
    assert_eq!(composed.len(), 3);
    assert!(!contains(&composed, START + 21_777));
    assert!(!contains(&composed, START + 40_000));
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/decisions.json")).unwrap();
    let mut request: Request = serde_json::from_value(fixture["base"].clone()).unwrap();
    request.assignment.valid_from_ms = START;
    request.assignment.expires_at_ms = START + 60_000;
    request.assignment.goals.truncate(1);
    request.assignment.goals[0].eligible_windows = result.windows.clone();
    request.assignment.goals[0].exposure_ms = 20_000;
    request.assignment.goals[0].overhead_ms = 10_000;
    request.state.now_ms = START;
    request.state.conditions_valid_until_ms = START + 60_001;
    // Thirty seconds cannot bridge the obstruction; the later clear span fits.
    assert!(matches!(evaluate(&request), Ok(Decision::Wait { .. })));
    request.state.meridian_exclusion = policy;
    request.assignment.goals[0].transits = Some(transits);
    assert!(matches!(evaluate(&request), Ok(Decision::CheckIn { .. })));
    request.state.meridian_exclusion = MeridianExclusion {
        before_ms: 0,
        after_ms: 0,
    };
    request.assignment.goals[0].transits = None;
    request.state.now_ms = result.windows[1].start_ms;
    assert!(matches!(evaluate(&request), Ok(Decision::Acquire { .. })));
    request.state.now_ms = result.windows[1].end_ms - 29_999;
    assert!(matches!(evaluate(&request), Ok(Decision::CheckIn { .. })));
}

#[test]
fn horizon_blocks_large_regions_without_exhausting_the_search() {
    let (target, site, eop, limits) = inputs();
    let horizon = Horizon::Custom {
        points: vec![
            HorizonPoint {
                azimuth_degrees: 0.0,
                altitude_degrees: 80.0,
            },
            HorizonPoint {
                azimuth_degrees: 360.0,
                altitude_degrees: 80.0,
            },
        ],
    };
    let result =
        altitude_windows(target, site, eop, &horizon, limits, interval(86_400_000)).unwrap();
    assert!(result.windows.is_empty());
    assert!(result.unresolved.is_empty());
}

#[test]
fn invalid_or_stale_coverage_never_returns_partial_windows() {
    let (target, site, eop, limits) = inputs();
    let horizon = Horizon::FixedMinimum {};
    assert_eq!(
        altitude_windows(target, site, eop, &horizon, limits, interval(0)),
        Err(VisibilityError::InvalidSpan)
    );
    assert_eq!(
        altitude_windows(
            target,
            site,
            EarthOrientation {
                valid_until_ms: START + 1000,
                ..eop
            },
            &horizon,
            limits,
            interval(1000)
        ),
        Err(VisibilityError::StaleEarthOrientation)
    );
    let start = 1_483_228_799_000;
    assert_eq!(
        altitude_windows(
            target,
            site,
            EarthOrientation {
                valid_from_ms: start,
                valid_until_ms: start + 3000,
                ..eop
            },
            &horizon,
            limits,
            Interval {
                start_ms: start,
                end_ms: start + 2000
            }
        ),
        Err(VisibilityError::EarthOrientationDiscontinuity)
    );
}

#[test]
fn whole_day_windows_are_ordered_and_dense_checks_never_find_false_clearance() {
    let (target, site, eop, mut limits) = inputs();
    limits.rig_minimum_degrees = 20.0;
    limits.rig_maximum_degrees = 50.0;
    for latitude in [-70.0, -35.0, 0.0, 35.0, 70.0] {
        let site = Site {
            latitude_degrees: latitude,
            ..site
        };
        let result = altitude_windows(
            target,
            site,
            eop,
            &Horizon::FixedMinimum {},
            limits,
            interval(86_400_000),
        )
        .unwrap();
        assert!(result
            .windows
            .windows(2)
            .all(|w| w[0].end_ms < w[1].start_ms));
        for window in &result.windows {
            for time in (window.start_ms..=window.end_ms)
                .step_by(30_000)
                .chain([window.end_ms])
            {
                let p = observe(target, site, eop, time).unwrap();
                assert!(altitude_allowed(
                    &Horizon::FixedMinimum {},
                    limits,
                    p.azimuth_degrees,
                    p.altitude_degrees
                )
                .unwrap());
            }
        }
    }
}
