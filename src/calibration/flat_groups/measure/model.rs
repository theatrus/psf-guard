//! Flat grouping is Seiza's native book; this module preserves host imports.
pub use seiza_calibration::flat_response::evidence::Side;
pub use seiza_calibration::flat_response::groups::{
    AlikeRange, AngleSpan, AngleTerm, Book, Boundary, Event, Family, HandEdge, ImagingSession,
    Limits, Placement, Session, ROTATION_CLUSTER_DEG,
};

/// Normalize a catalog filter label for display/lookups.
pub fn canonical_filter(filter: &str) -> String {
    filter.trim().to_ascii_lowercase()
}

/// Resolve catalog profile bindings, then let Seiza group the stored light rows.
pub fn imaging_sessions(
    lights: Vec<(i64, Option<String>, i64)>,
    rigs_of: impl Fn(Option<&str>) -> Vec<String>,
) -> anyhow::Result<Vec<ImagingSession>> {
    use seiza_calibration::flat_response::groups::{self, Request};
    let profiles: std::collections::BTreeSet<_> =
        lights.iter().map(|(_, p, _)| p.clone()).collect();
    let profile_rigs = profiles
        .into_iter()
        .map(|p| {
            let rigs = rigs_of(p.as_deref());
            (p, rigs)
        })
        .collect();
    Ok(groups::evaluate(Request {
        imaging_lights: lights,
        profile_rigs,
        ..Request::default()
    })?
    .imaging)
}
