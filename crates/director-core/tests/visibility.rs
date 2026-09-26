use psf_guard_director_core::visibility::*;
use serde::Deserialize;

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

fn limits() -> AltitudeLimits {
    AltitudeLimits {
        rig_minimum_degrees: 15.0,
        project_minimum_degrees: 20.0,
        horizon_offset_degrees: 5.0,
        rig_maximum_degrees: 85.0,
        project_maximum_degrees: 90.0,
    }
}

#[test]
fn native_sofa_reference_parity_across_sites_poles_and_leap_boundary() {
    #[derive(Deserialize)]
    struct Fixture {
        source: String,
        sha256: String,
        samples: Vec<Sample>,
    }
    #[derive(Deserialize)]
    struct Sample {
        unix_ms: u64,
        position: IcrsPosition,
        site: Site,
        orientation: EarthOrientation,
        expected: ObservedPosition,
    }
    let fixture: Fixture =
        serde_json::from_str(include_str!("fixtures/nina-sofa-positions.json")).unwrap();
    assert_eq!(fixture.source, "SOFA_2023_10_11.dll");
    assert_eq!(fixture.sha256.len(), 64);
    assert_eq!(fixture.samples.len(), 48);
    for sample in fixture.samples {
        let actual = observe(
            sample.position,
            sample.site,
            sample.orientation,
            sample.unix_ms,
        )
        .unwrap();
        for (a, b) in [
            (actual.azimuth_degrees, sample.expected.azimuth_degrees),
            (actual.altitude_degrees, sample.expected.altitude_degrees),
            (
                actual.hour_angle_degrees,
                sample.expected.hour_angle_degrees,
            ),
        ] {
            assert!(
                (a - b).abs() < 1e-9,
                "{actual:?} != {:?} at {} / {:?}",
                sample.expected,
                sample.unix_ms,
                sample.site
            );
        }
    }
}

#[test]
fn native_wrap_and_explicit_endpoint_discontinuity() {
    let horizon = curve(&[(0.0, 10.0), (90.0, 30.0), (360.0, 40.0)]);
    assert_eq!(horizon.altitude(45.0), Ok(Some(20.0)));
    assert_eq!(horizon.altitude(360.0), Ok(Some(10.0)));
    assert_eq!(horizon.altitude(720.0), Ok(Some(10.0)));
    assert_eq!(horizon.altitude(-270.0), Ok(Some(30.0)));
    assert_eq!(horizon.altitude(-1e-300), Ok(Some(40.0)));
    assert!(horizon.altitude(359.999).unwrap().unwrap() > 39.99);
}

#[test]
fn narrow_obstructions_and_adjacent_doubles_survive() {
    let horizon = curve(&[
        (0.0, 0.0),
        (100.0, 0.0),
        (100.00000000000001, 80.0),
        (100.00000000000003, 0.0),
        (360.0, 0.0),
    ]);
    assert_eq!(horizon.altitude(100.00000000000001), Ok(Some(80.0)));
    assert!(!altitude_allowed(&horizon, limits(), 100.00000000000001, 75.0).unwrap());
    assert!(altitude_allowed(&horizon, limits(), 100.0, 75.0).unwrap());
}

#[test]
fn project_preferences_cannot_relax_rig_or_custom_horizon() {
    let horizon = curve(&[(0.0, 30.0), (360.0, 30.0)]);
    let mut policy = limits();
    policy.project_minimum_degrees = -10.0;
    assert!(!altitude_allowed(&horizon, policy, 0.0, 35.0).unwrap());
    assert!(altitude_allowed(&horizon, policy, 0.0, 35.0001).unwrap());
    assert!(!altitude_allowed(&horizon, policy, 0.0, 85.0).unwrap());
    policy.horizon_offset_degrees = -1.0;
    assert_eq!(
        altitude_allowed(&horizon, policy, 0.0, 40.0),
        Err(VisibilityError::InvalidAltitudeLimits)
    );
}

#[test]
fn fixed_minimum_is_not_an_implicit_zero_degree_curve() {
    let horizon = Horizon::FixedMinimum {};
    assert_eq!(horizon.altitude(0.0), Ok(None));
    let policy = AltitudeLimits {
        rig_minimum_degrees: -10.0,
        project_minimum_degrees: -20.0,
        horizon_offset_degrees: 5.0,
        rig_maximum_degrees: 85.0,
        project_maximum_degrees: 90.0,
    };
    assert!(altitude_allowed(&horizon, policy, 0.0, -5.0).unwrap());
    assert!(!altitude_allowed(&horizon, policy, 0.0, -10.0).unwrap());
}

#[test]
fn invalid_horizon_shapes_and_nonfinite_values_fail() {
    for points in [
        vec![],
        vec![(0.0, 0.0)],
        vec![(10.0, 0.0), (360.0, 0.0)],
        vec![(0.0, 0.0), (359.0, 0.0)],
        vec![(0.0, 0.0), (90.0, 5.0), (90.0, 10.0), (360.0, 0.0)],
        vec![(0.0, f64::NAN), (360.0, 0.0)],
        vec![(0.0, 0.0), (360.0, 91.0)],
    ] {
        assert_eq!(
            curve(&points).altitude(0.0),
            Err(VisibilityError::InvalidHorizon)
        );
    }
    assert_eq!(
        Horizon::FixedMinimum {}.altitude(f64::NAN),
        Err(VisibilityError::InvalidPosition)
    );
    assert_eq!(
        curve(&vec![(0.0, 0.0); 4099]).validate(),
        Err(VisibilityError::InvalidHorizon)
    );
}

#[test]
fn typed_inputs_reject_unknown_fields_and_missing_modes() {
    assert!(serde_json::from_str::<Horizon>(r#"{"mode":"fixed_minimum","points":[]}"#).is_err());
    assert!(serde_json::from_str::<Horizon>(r#"{"points":[]}"#).is_err());
    assert!(serde_json::from_str::<Horizon>(
        r#"{"mode":"custom","points":[{"azimuth_degrees":0,"altitude_degrees":0,"extra":1}]}"#
    )
    .is_err());
}

fn observation() -> (IcrsPosition, Site, EarthOrientation, u64) {
    let time = 1_790_409_600_000;
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
            valid_from_ms: time,
            valid_until_ms: time + 1000,
        },
        time,
    )
}

#[test]
fn orientation_validity_is_half_open_and_never_implicitly_zero() {
    let (position, site, orientation, time) = observation();
    assert!(observe(position, site, orientation, time).is_ok());
    assert_eq!(
        observe(position, site, orientation, time - 1),
        Err(VisibilityError::StaleEarthOrientation)
    );
    assert_eq!(
        observe(position, site, orientation, time + 1000),
        Err(VisibilityError::StaleEarthOrientation)
    );
    assert_eq!(
        observe(
            position,
            site,
            EarthOrientation {
                ut1_minus_utc_seconds: f64::NAN,
                ..orientation
            },
            time
        ),
        Err(VisibilityError::InvalidEarthOrientation)
    );
    assert_eq!(
        observe(
            position,
            site,
            EarthOrientation {
                valid_until_ms: time,
                ..orientation
            },
            time
        ),
        Err(VisibilityError::InvalidEarthOrientation)
    );
}

#[test]
fn invalid_coordinates_and_unbounded_time_never_reach_dependency_panics() {
    let (position, site, orientation, time) = observation();
    for ra in [f64::NAN, f64::INFINITY, -1.0, 360.0] {
        assert_eq!(
            observe(
                IcrsPosition {
                    ra_degrees: ra,
                    ..position
                },
                site,
                orientation,
                time
            ),
            Err(VisibilityError::InvalidPosition)
        );
    }
    assert_eq!(
        observe(
            position,
            Site {
                latitude_degrees: 91.0,
                ..site
            },
            orientation,
            time
        ),
        Err(VisibilityError::InvalidSite)
    );
    let unbounded = EarthOrientation {
        valid_from_ms: 0,
        valid_until_ms: u64::MAX,
        ..orientation
    };
    assert_eq!(
        observe(position, site, unbounded, u64::MAX - 1),
        Err(VisibilityError::UnsupportedTime)
    );
    // The translation suppresses SOFA's dubious-year warning. The wrapper
    // supplies an explicit cutoff rather than trusting stale leap-second data.
    assert_eq!(
        observe(position, site, unbounded, 1_861_920_000_000),
        Err(VisibilityError::UnsupportedTime)
    );
}

#[test]
fn never_confuse_negative_altitude_with_missing_geometry() {
    let (position, site, orientation, time) = observation();
    let actual = observe(
        IcrsPosition {
            dec_degrees: -89.9,
            ..position
        },
        site,
        orientation,
        time,
    )
    .unwrap();
    assert!(actual.altitude_degrees < 0.0);
    assert!(!altitude_allowed(
        &Horizon::FixedMinimum {},
        limits(),
        actual.azimuth_degrees,
        actual.altitude_degrees
    )
    .unwrap());
}

#[test]
fn conflicting_valid_preferences_have_empty_visibility_not_invalid_geometry() {
    let policy = AltitudeLimits {
        rig_maximum_degrees: 60.0,
        project_minimum_degrees: 70.0,
        ..limits()
    };
    for altitude in [50.0, 65.0, 75.0, 80.0] {
        assert_eq!(
            altitude_allowed(&Horizon::FixedMinimum {}, policy, 0.0, altitude),
            Ok(false)
        );
    }
}
