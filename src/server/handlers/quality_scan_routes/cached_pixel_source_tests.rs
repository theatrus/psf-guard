use super::cached_pixel_source_matches;

#[test]
fn a_rotator_nina_reports_as_absent_has_no_sky_angle() {
    assert_eq!(
        super::rotator_position_from_metadata(
            r#"{"RotatorPosition": 179.99, "RotatorMechanicalPosition": 168.5}"#
        ),
        Some(179.99)
    );
    assert_eq!(
        super::rotator_position_from_metadata(r#"{"RotatorPosition": 90.0}"#),
        Some(90.0)
    );
    // N.I.N.A. writes the lowest double as the mechanical position when no
    // rotator is connected, beside a sky angle of 0 that means nothing.
    assert_eq!(
        super::rotator_position_from_metadata(
            r#"{"RotatorPosition": 0.0, "RotatorMechanicalPosition": -1.7976931348623157e308}"#
        ),
        None
    );
    assert_eq!(
        super::rotator_position_from_metadata(r#"{"HFR": 2.1}"#),
        None
    );
}

#[test]
fn mapped_evidence_requires_the_exact_registered_source_path() {
    let directory = tempfile::tempdir().unwrap();
    let current = directory.path().join("current").join("frame.fits");
    let old = directory.path().join("old").join("frame.fits");

    assert!(cached_pixel_source_matches(
        &current.to_string_lossy(),
        "frame.fits",
        Some(&current)
    ));
    assert!(!cached_pixel_source_matches(
        &old.to_string_lossy(),
        "frame.fits",
        Some(&current)
    ));
    assert!(cached_pixel_source_matches(
        &old.to_string_lossy(),
        "frame.fits",
        None
    ));
}
