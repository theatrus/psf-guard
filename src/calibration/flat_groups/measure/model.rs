//! Flat grouping's model is Seiza's book, shared with the desktop apps.
pub use seiza_calibration::flat_response::evidence::Side;
pub use seiza_calibration::flat_response::groups::{
    canonical_filter, imaging_sessions, AlikeRange, AngleSpan, AngleTerm, Book, Boundary, Event,
    Family, HandEdge, ImagingSession, Limits, Placement, Session, ROTATION_CLUSTER_DEG,
};
