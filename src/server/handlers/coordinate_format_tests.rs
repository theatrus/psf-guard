use super::format_coordinates;

#[test]
fn target_scheduler_ra_is_already_in_hours() {
    assert_eq!(
        format_coordinates(Some(10.5), Some(-20.25)).as_deref(),
        Some("RA 10h 30m 00.0s, Dec -20° 15' 00.0\"")
    );
}
