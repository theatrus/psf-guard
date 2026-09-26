use psf_guard_director_core::{visibility::*, windows::Interval};

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
fn curve(points: &[(f64, f64)]) -> Horizon {
    Horizon::Custom {
        points: points
            .iter()
            .map(|&(azimuth_degrees, altitude_degrees)| HorizonPoint {
                azimuth_degrees,
                altitude_degrees,
            })
            .collect(),
    }
}

#[test]
fn ordinary_exposure_and_overhead_can_be_proven_clear() {
    let (target, site, eop, limits) = inputs();
    for duration in [1, 1000, 60_000, 3_600_000, 86_400_000] {
        assert_eq!(
            check_altitude_span(
                target,
                site,
                eop,
                &Horizon::FixedMinimum {},
                limits,
                interval(duration)
            ),
            Ok(AltitudeSpan::Clear)
        );
    }
}

#[test]
fn clear_endpoints_do_not_authorize_a_mid_exposure_obstruction() {
    let (target, site, eop, limits) = inputs();
    let middle = observe(target, site, eop, START + 30_000).unwrap();
    let az = middle.azimuth_degrees;
    let horizon = curve(&[
        (0.0, -89.0),
        (az - 0.0001, -89.0),
        (az, 89.0),
        (az + 0.0001, -89.0),
        (360.0, -89.0),
    ]);
    for time in [START, START + 60_000] {
        let point = observe(target, site, eop, time).unwrap();
        assert!(altitude_allowed(
            &horizon,
            limits,
            point.azimuth_degrees,
            point.altitude_degrees
        )
        .unwrap());
    }
    assert_eq!(
        check_altitude_span(target, site, eop, &horizon, limits, interval(60_000)),
        Ok(AltitudeSpan::Blocked {
            at_ms: START + 30_000
        })
    );
}

#[test]
fn arbitrarily_narrow_unsampled_obstruction_is_never_declared_clear() {
    let (target, site, eop, limits) = inputs();
    let az = observe(target, site, eop, START + 21_777)
        .unwrap()
        .azimuth_degrees;
    let horizon = curve(&[
        (0.0, -89.0),
        (az.next_down(), -89.0),
        (az, 89.0),
        (az.next_up(), -89.0),
        (360.0, -89.0),
    ]);
    assert!(matches!(
        check_altitude_span(target, site, eop, &horizon, limits, interval(60_000)),
        Ok(AltitudeSpan::Blocked { .. }) | Err(VisibilityError::UnresolvedSpan)
    ));
}

#[test]
fn minimum_crossing_and_extra_overhead_invalidate_start_only_clearance() {
    let (target, site, eop, mut limits) = inputs();
    let first = observe(target, site, eop, START).unwrap().altitude_degrees;
    let last = observe(target, site, eop, START + 60_000)
        .unwrap()
        .altitude_degrees;
    let between = (first + last) / 2.0;
    if first > last {
        limits.rig_minimum_degrees = between;
    } else {
        limits.rig_maximum_degrees = between;
    }
    assert_eq!(
        check_altitude_span(
            target,
            site,
            eop,
            &Horizon::FixedMinimum {},
            limits,
            interval(10_000)
        ),
        Ok(AltitudeSpan::Clear)
    );
    assert_eq!(
        check_altitude_span(
            target,
            site,
            eop,
            &Horizon::FixedMinimum {},
            limits,
            interval(60_000)
        ),
        Ok(AltitudeSpan::Blocked {
            at_ms: START + 60_000
        })
    );
}

#[test]
fn maximum_altitude_crossing_is_rejected_even_when_both_endpoints_are_clear() {
    let (mut target, site, eop, mut limits) = inputs();
    let site = Site {
        latitude_degrees: 0.0,
        ..site
    };
    target.dec_degrees = 0.0;
    let middle = START + 1_800_000;
    for _ in 0..5 {
        let p = observe(target, site, eop, middle).unwrap();
        target.ra_degrees = (target.ra_degrees + p.hour_angle_degrees).rem_euclid(360.0);
    }
    limits.rig_maximum_degrees = observe(target, site, eop, middle).unwrap().altitude_degrees - 1.0;
    let horizon = Horizon::FixedMinimum {};
    for time in [START, START + 3_600_000] {
        let p = observe(target, site, eop, time).unwrap();
        assert!(altitude_allowed(&horizon, limits, p.azimuth_degrees, p.altitude_degrees).unwrap());
    }
    assert_eq!(
        check_altitude_span(target, site, eop, &horizon, limits, interval(3_600_000)),
        Ok(AltitudeSpan::Blocked { at_ms: middle })
    );
}

#[test]
fn validity_covers_finish_and_invalid_requests_fail_before_blocked_result() {
    let (target, site, eop, limits) = inputs();
    let horizon = Horizon::FixedMinimum {};
    for duration in [0, 86_400_001] {
        assert_eq!(
            check_altitude_span(target, site, eop, &horizon, limits, interval(duration)),
            Err(VisibilityError::InvalidSpan)
        );
    }
    assert_eq!(
        check_altitude_span(
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
    assert_eq!(
        check_altitude_span(
            target,
            site,
            eop,
            &horizon,
            limits,
            Interval {
                start_ms: START,
                end_ms: START - 1
            }
        ),
        Err(VisibilityError::InvalidSpan)
    );
    let mut bad = limits;
    bad.horizon_offset_degrees = f64::NAN;
    assert_eq!(
        check_altitude_span(target, site, eop, &horizon, bad, interval(1000)),
        Err(VisibilityError::InvalidAltitudeLimits)
    );
}

#[test]
fn near_boundary_clear_samples_are_not_a_clear_span() {
    let (target, site, eop, mut limits) = inputs();
    let altitude = observe(target, site, eop, START).unwrap().altitude_degrees;
    limits.rig_minimum_degrees = altitude - 0.00001;
    assert_eq!(
        check_altitude_span(
            target,
            site,
            eop,
            &Horizon::FixedMinimum {},
            limits,
            interval(1)
        ),
        Err(VisibilityError::UnresolvedSpan)
    );
}

#[test]
fn clear_spans_agree_with_dense_sofa_checks_across_sites_and_polar_targets() {
    let (_, site, eop, limits) = inputs();
    let horizon = curve(&[
        (0.0, -40.0),
        (10.0, 0.0),
        (90.0, -30.0),
        (170.0, 20.0),
        (270.0, -20.0),
        (360.0, 10.0),
    ]);
    let mut clear = 0;
    for latitude in [-80.0, 0.0, 35.0, 80.0] {
        for dec in [-89.9, -20.0, 20.0, 89.9] {
            for ra in [0.0, 83.0, 180.0, 359.999] {
                let target = IcrsPosition {
                    ra_degrees: ra,
                    dec_degrees: dec,
                };
                let site = Site {
                    latitude_degrees: latitude,
                    ..site
                };
                if check_altitude_span(target, site, eop, &horizon, limits, interval(300_000))
                    == Ok(AltitudeSpan::Clear)
                {
                    clear += 1;
                    for second in 0..=300 {
                        let p = observe(target, site, eop, START + second * 1000).unwrap();
                        assert!(altitude_allowed(
                            &horizon,
                            limits,
                            p.azimuth_degrees,
                            p.altitude_degrees
                        )
                        .unwrap());
                    }
                }
            }
        }
    }
    assert!(clear >= 10);
}

#[test]
fn constant_eop_cannot_authorize_crossing_a_leap_boundary() {
    let (target, site, _, limits) = inputs();
    let start = 1_483_228_799_000;
    for dut1 in [-1.0, 1.0] {
        let eop = EarthOrientation {
            ut1_minus_utc_seconds: dut1,
            polar_motion_x_radians: 0.001,
            polar_motion_y_radians: -0.001,
            valid_from_ms: start,
            valid_until_ms: start + 2001,
        };
        assert_eq!(
            check_altitude_span(
                target,
                site,
                eop,
                &Horizon::FixedMinimum {},
                limits,
                Interval {
                    start_ms: start,
                    end_ms: start + 2000
                }
            ),
            Err(VisibilityError::EarthOrientationDiscontinuity)
        );
        assert_eq!(
            check_altitude_span(
                target,
                site,
                eop,
                &Horizon::FixedMinimum {},
                limits,
                Interval {
                    start_ms: start,
                    end_ms: start + 999
                }
            ),
            Ok(AltitudeSpan::Clear)
        );
    }
}
