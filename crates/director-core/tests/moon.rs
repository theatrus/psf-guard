use psf_guard_director_core::moon::{MoonError, MoonEvidence, MoonPolicy, MoonReason};

fn policy() -> MoonPolicy {
    MoonPolicy {
        enabled: true,
        ..MoonPolicy::default()
    }
}
fn evidence(phase: f64, altitude: f64, separation: f64) -> MoonEvidence {
    MoonEvidence {
        days_from_full: phase,
        altitude_degrees: altitude,
        separation_degrees: separation,
    }
}

#[test]
fn full_moon_and_half_width_preserve_lorentzian_units() {
    let p = policy();
    assert!(!p.evaluate(evidence(0.0, 30.0, 59.99)).unwrap().allowed);
    assert!(p.evaluate(evidence(0.0, 30.0, 60.0)).unwrap().allowed);
    let half = p.evaluate(evidence(7.0, 30.0, 30.0)).unwrap();
    assert_eq!(half.required_separation_degrees, 30.0);
    assert!(half.allowed);
}

#[test]
fn relaxation_changes_width_and_separation_without_nan_at_the_boundary() {
    let p = MoonPolicy {
        relax_degrees_per_degree: 2.0,
        ..policy()
    };
    let half = p.evaluate(evidence(3.5, -5.0, 20.0)).unwrap();
    assert_eq!(half.required_separation_degrees, 20.0);
    assert!(half.allowed);
    assert_eq!(
        p.evaluate(evidence(0.0, -15.0, 0.0)).unwrap().reason,
        MoonReason::RelaxedBelowMinimum
    );
    assert!(!p.evaluate(evidence(0.0, -14.999, 0.0)).unwrap().allowed);
}

#[test]
fn moon_down_uses_the_configured_boundary_even_when_separation_is_zero() {
    let p = MoonPolicy {
        moon_down: true,
        separation_degrees: 0.0,
        ..policy()
    };
    assert_eq!(
        p.evaluate(evidence(14.75, 5.0, 180.0)).unwrap().reason,
        MoonReason::MoonMustBeDown
    );
    assert!(p.evaluate(evidence(14.75, 4.999, 0.0)).unwrap().allowed);
}

#[test]
fn disabled_policy_and_zero_relaxation_have_distinct_meaning() {
    assert!(
        MoonPolicy::default()
            .evaluate(evidence(f64::NAN, f64::NAN, f64::NAN))
            .unwrap()
            .allowed
    );
    assert!(
        !policy()
            .evaluate(evidence(0.0, -30.0, 0.0))
            .unwrap()
            .allowed
    );
    let relaxed = MoonPolicy {
        relax_degrees_per_degree: 5.0,
        ..policy()
    };
    assert_eq!(
        relaxed.evaluate(evidence(0.0, -10.0, 0.0)).unwrap().reason,
        MoonReason::RelaxedSeparation
    );
}

#[test]
fn invalid_policy_and_evidence_cannot_be_treated_as_clear() {
    for p in [
        MoonPolicy {
            width_days: 0.0,
            ..policy()
        },
        MoonPolicy {
            separation_degrees: f64::NAN,
            ..policy()
        },
        MoonPolicy {
            relax_min_altitude_degrees: 5.0,
            ..policy()
        },
    ] {
        assert_eq!(
            p.evaluate(evidence(0.0, 0.0, 180.0)),
            Err(MoonError::InvalidPolicy)
        );
    }
    for e in [
        evidence(f64::NAN, 0.0, 180.0),
        evidence(15.0, 0.0, 180.0),
        evidence(0.0, 91.0, 180.0),
        evidence(0.0, 0.0, -1.0),
    ] {
        assert_eq!(policy().evaluate(e), Err(MoonError::InvalidEvidence));
    }
}

#[test]
fn eligible_sensitive_filters_keep_their_static_aversion() {
    let ha = MoonPolicy {
        separation_degrees: 30.0,
        width_days: 3.0,
        ..policy()
    };
    let luminance = MoonPolicy {
        separation_degrees: 90.0,
        width_days: 10.0,
        ..policy()
    };
    let sky = evidence(0.0, 30.0, 60.0);
    assert!(ha.evaluate(sky).unwrap().allowed);
    assert!(!luminance.evaluate(sky).unwrap().allowed);
    assert!(luminance.aversion().unwrap() > ha.aversion().unwrap());
    assert_eq!(
        MoonPolicy {
            moon_down: true,
            ..policy()
        }
        .aversion()
        .unwrap(),
        1.0
    );
}

#[test]
fn whole_span_windows_remain_safe_between_samples_and_at_lunar_rise() {
    use psf_guard_director_core::moon::moon_windows;
    use psf_guard_director_core::{ephemeris::*, visibility::*, windows::Interval};
    let start = 1_790_409_600_000;
    let span = Interval {
        start_ms: start,
        end_ms: start + 86_400_000,
    };
    let site = Site {
        latitude_degrees: 35.0,
        longitude_degrees: -120.0,
        elevation_meters: 1000.0,
    };
    let orientation = EarthOrientation {
        ut1_minus_utc_seconds: 0.0,
        polar_motion_x_radians: 0.0,
        polar_motion_y_radians: 0.0,
        valid_from_ms: start,
        valid_until_ms: span.end_ms + 1,
    };
    let target = IcrsPosition {
        ra_degrees: 83.0,
        dec_degrees: -5.0,
    };
    for policy in [
        MoonPolicy {
            moon_down: true,
            separation_degrees: 0.0,
            ..policy()
        },
        MoonPolicy {
            relax_degrees_per_degree: 2.0,
            ..policy()
        },
    ] {
        let windows = moon_windows(&policy, target, site, orientation, span).unwrap();
        assert!(!windows.is_empty());
        if policy.moon_down {
            assert!(windows.iter().map(|w| w.end_ms - w.start_ms).sum::<u64>() < 86_400_000);
        }
        for window in windows {
            for now in (window.start_ms..window.end_ms).step_by(5_000) {
                let moon = moon_position(now);
                let sun = sun_position(now).unwrap();
                let evidence = MoonEvidence {
                    days_from_full: (180.0 - separation_degrees(sun, moon)) * 29.5 / 360.0,
                    altitude_degrees: observe(moon, site, orientation, now)
                        .unwrap()
                        .altitude_degrees,
                    separation_degrees: separation_degrees(target, moon),
                };
                assert!(
                    policy.evaluate(evidence).unwrap().allowed,
                    "unsafe cell at {now}"
                );
            }
        }
    }
}
