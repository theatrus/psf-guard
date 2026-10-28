//! Shared response-family and angle-epoch policy.
pub use seiza_calibration::flat_response::groups::{
    AngleEpochs, Epoch, FamilyBasis, FamilyChoice, ANGLE_PRIOR_DEG, EPOCH_JUMP_DEG,
    FAMILY_EVIDENCE_VERSION,
};

/// Signed circular angle for catalog/solve coordinate conversion.
pub fn wrap(difference: f64) -> f64 {
    let d = difference.rem_euclid(360.0);
    if d > 180.0 {
        d - 360.0
    } else {
        d
    }
}
/// Sky/mechanical conversion modulo a meridian flip.
pub fn half(difference: f64) -> f64 {
    let d = difference.rem_euclid(180.0);
    if d > 90.0 {
        d - 180.0
    } else {
        d
    }
}
