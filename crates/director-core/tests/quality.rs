use psf_guard_director_core::{quality::*, recovery::Verdict};

fn fixture() -> (Policy, Reference, Frame) {
    let frame = Frame {
        capture_id: "reference-capture".into(),
        observed_at_ms: 1000,
        context: Context {
            rig_id: "rig".into(),
            configuration_id: "config".into(),
            target_id: "target".into(),
            recipe_fingerprint: "recipe".into(),
            analysis_fingerprint: "nina-fast-settings-1".into(),
            width: 2000,
            height: 1000,
        },
        metrics: Metrics {
            stars: Some(100),
            hfr_pixels: Some(2.0),
            background_adu: Some(1000.0),
            eccentricity: Some(0.4),
        },
    };
    let reference = Reference {
        id: "approved-reference".into(),
        approved: true,
        initial_group: vec![],
        frame: frame.clone(),
    };
    let frame = Frame {
        capture_id: "new-capture".into(),
        observed_at_ms: 2000,
        ..frame
    };
    (Policy::default(), reference, frame)
}

fn initial_group() -> Vec<Frame> {
    let (_, reference, _) = fixture();
    (0..5)
        .map(|n| Frame {
            capture_id: format!("initial-{n}"),
            observed_at_ms: 1000 + n * 100,
            ..reference.frame.clone()
        })
        .collect()
}

#[test]
fn stable_initial_group_is_frozen_but_always_warns_that_quality_is_unknown() {
    let (p, _, mut frame) = fixture();
    let group = initial_group();
    let baseline = build_initial_reference(&p, "baseline", &group).unwrap();
    assert!(!baseline.approved);
    assert_eq!(baseline.initial_group, group);
    let assessed = classify(&p, &baseline, &frame, 2000).unwrap();
    assert_eq!(assessed.verdict, Verdict::ConfirmedGood);
    assert!(assessed.reference_quality_unknown);
    let original = baseline.clone();
    frame.metrics.stars = Some(20);
    frame.metrics.background_adu = Some(2000.0);
    assert_eq!(
        classify(&p, &baseline, &frame, 2000).unwrap().verdict,
        Verdict::CorroboratedPoor
    );
    assert_eq!(baseline, original);
    let first = &group[0];
    assert_eq!(
        classify(&p, &baseline, first, 2000).unwrap().reason,
        Reason::SameCapture
    );
}

#[test]
fn initial_group_rejects_incomplete_incompatible_unstable_and_replayed_evidence() {
    let p = Policy::default();
    assert_eq!(
        build_initial_reference(&p, "baseline", &initial_group()[..4]),
        Err(Error::InsufficientSamples)
    );
    for fault in 0..7 {
        let mut group = initial_group();
        match fault {
            0 => group[2].metrics.stars = Some(40),
            1 => group[2].metrics.background_adu = Some(2000.0),
            2 => group[2].metrics.hfr_pixels = Some(3.0),
            3 => group[2].metrics.eccentricity = None,
            4 => group[2].context.recipe_fingerprint = "other".into(),
            5 => group[2].capture_id = group[1].capture_id.clone(),
            _ => group[2].observed_at_ms = group[1].observed_at_ms,
        }
        assert!(
            build_initial_reference(&p, "baseline", &group).is_err(),
            "fault {fault}"
        );
    }
}

#[test]
fn stable_cloudy_start_is_not_claimed_to_be_known_good() {
    let p = Policy::default();
    let mut group = initial_group();
    for f in &mut group {
        f.metrics.stars = Some(25);
        f.metrics.background_adu = Some(5000.0);
    }
    let baseline = build_initial_reference(&p, "cloudy-start", &group).unwrap();
    let next = Frame {
        capture_id: "next".into(),
        observed_at_ms: 2000,
        ..group.last().unwrap().clone()
    };
    let result = classify(&p, &baseline, &next, 2000).unwrap();
    assert_eq!(result.reason, Reason::ConsistentWithReference);
    assert!(result.reference_quality_unknown);
}

#[test]
fn requires_two_signals_and_keeps_hysteresis() {
    let (p, r, mut f) = fixture();
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().verdict,
        Verdict::ConfirmedGood
    );
    f.metrics.stars = Some(20);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().verdict,
        Verdict::Unknown
    );
    f.metrics.background_adu = Some(2000.0);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().verdict,
        Verdict::CorroboratedPoor
    );
    f.metrics.stars = Some(70);
    f.metrics.background_adu = Some(1300.0);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().verdict,
        Verdict::Unknown
    );
    f.metrics.stars = Some(90);
    f.metrics.background_adu = Some(1100.0);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().verdict,
        Verdict::ConfirmedGood
    );
}

#[test]
fn a_long_poor_run_cannot_adapt_the_reference() {
    let (p, r, mut f) = fixture();
    let original = r.clone();
    f.metrics.stars = Some(20);
    f.metrics.background_adu = Some(2000.0);
    for n in 0..100 {
        f.capture_id = format!("capture-{n}");
        f.observed_at_ms = 2000 + n * 1000;
        assert_eq!(
            classify(&p, &r, &f, f.observed_at_ms).unwrap().verdict,
            Verdict::CorroboratedPoor
        );
    }
    assert_eq!(original, r);
}

#[test]
fn focus_tracking_and_missing_data_do_not_become_clouds_or_recovery() {
    let (p, r, mut f) = fixture();
    f.metrics.stars = Some(20);
    f.metrics.background_adu = Some(2000.0);
    f.metrics.hfr_pixels = Some(4.0);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::FocusOrTracking
    );
    f.metrics.hfr_pixels = Some(2.0);
    f.metrics.eccentricity = Some(0.8);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::FocusOrTracking
    );
    f.metrics.eccentricity = None;
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::MissingMetrics
    );
}

#[test]
fn context_changes_cannot_pause_or_resume_another_setup() {
    let (p, r, original) = fixture();
    for change in 0..7 {
        let mut f = original.clone();
        match change {
            0 => f.context.rig_id = "another".into(),
            1 => f.context.configuration_id = "another".into(),
            2 => f.context.target_id = "another".into(),
            3 => f.context.recipe_fingerprint = "another".into(),
            4 => f.context.analysis_fingerprint = "another".into(),
            5 => f.context.width += 1,
            _ => f.context.height += 1,
        }
        assert_eq!(
            classify(&p, &r, &f, 2000).unwrap().reason,
            Reason::IncompatibleContext
        );
    }
}

#[test]
fn stale_reference_replayed_and_future_observations_are_unknown() {
    let (p, r, mut f) = fixture();
    assert_eq!(
        classify(&p, &r, &f, 1999).unwrap().reason,
        Reason::StaleEvidence
    );
    assert_eq!(
        classify(&p, &r, &f, 32_001).unwrap().reason,
        Reason::StaleEvidence
    );
    f.observed_at_ms = p.reference_max_age_ms + 1001;
    assert_eq!(
        classify(&p, &r, &f, f.observed_at_ms).unwrap().reason,
        Reason::ReferenceExpired
    );
    f.observed_at_ms = 1000;
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::StaleEvidence
    );
    f.capture_id = r.frame.capture_id.clone();
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::SameCapture
    );
}

#[test]
fn reference_needs_approval_and_enough_well_shaped_stars() {
    let (p, mut r, f) = fixture();
    r.approved = false;
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::ReferenceUnapproved
    );
    r.approved = true;
    r.frame.metrics.stars = Some(2);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::InsufficientReference
    );
    r.frame.metrics.stars = Some(100);
    r.frame.metrics.eccentricity = Some(0.9);
    assert_eq!(
        classify(&p, &r, &f, 2000).unwrap().reason,
        Reason::InsufficientReference
    );
}

#[test]
fn nonfinite_and_invalid_data_fail_closed() {
    let (mut p, r, mut f) = fixture();
    for bad in [f64::NAN, f64::INFINITY, -1.0, 0.0] {
        f.metrics.background_adu = Some(bad);
        assert_eq!(classify(&p, &r, &f, 2000), Err(Error::InvalidFrame));
    }
    f.metrics.background_adu = Some(1000.0);
    p.good_star_ratio = p.poor_star_ratio;
    assert_eq!(classify(&p, &r, &f, 2000), Err(Error::InvalidPolicy));
}

#[test]
fn a_sparse_initial_group_is_insufficient_evidence_not_corrupt_input() {
    let (policy, _, _) = fixture();
    let mut frames = initial_group();
    frames.last_mut().unwrap().observed_at_ms += 7_200_001;
    assert_eq!(
        build_initial_reference(&policy, "sparse", &frames),
        Err(Error::UnstableBaseline)
    );
}
