//! Where a shot panel really is: the center, camera angle and field its
//! frames were taken with, read from the frames themselves. A locked panel
//! follows its original shots, not the camera the plan uses now.
//!
//! The source says how much the numbers prove. Only fresh pixel-derived
//! solves are evidence of where the pixels point; a FITS header or the plan
//! is a prediction, kept for display and labelled as such.

use super::{PanelSize, TangentPlane};
use crate::visibility::IcrsPosition;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MeasuredSource {
    /// Fresh plate solves of the frames' pixels.
    Solved,
    /// The frames' FITS headers or rotator readings.
    Header,
    /// The plan's own target and the rig's field: nothing measured.
    Plan,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MeasuredGeometry {
    /// Where the frames were shot, the median over them.
    pub center: IcrsPosition,
    /// The camera's up as shot, east of north at `center`.
    pub position_angle_degrees: f64,
    /// The field the frames cover, width along the camera's right.
    pub size: PanelSize,
    /// Where the center and angle come from.
    pub source: MeasuredSource,
    /// Where the size comes from.
    pub size_source: MeasuredSource,
    /// How many frames the center and angle were read from.
    pub frames: u32,
}

/// Readings closer than these are one place: see
/// [`MeasuredGeometry::same_place`].
pub const SAME_CENTER_ARCMIN: f64 = 0.5;
pub const SAME_ANGLE_DEGREES: f64 = 0.05;
pub const SAME_SIDE_SHARE: f64 = 1e-3;

fn valid_position(position: &IcrsPosition) -> bool {
    position.ra_degrees.is_finite()
        && (0.0..360.0).contains(&position.ra_degrees)
        && position.dec_degrees.is_finite()
        && (-90.0..=90.0).contains(&position.dec_degrees)
}

impl MeasuredGeometry {
    pub fn is_valid(&self) -> bool {
        valid_position(&self.center)
            && self.position_angle_degrees.is_finite()
            && (0.0..360.0).contains(&self.position_angle_degrees)
            && self.size.validate().is_ok()
    }

    /// One place for activation and for the panels around it: centers within
    /// half an arcminute, angles within a twentieth of a degree, sides within
    /// a thousandth. How many frames said so, and how sure they are, do not
    /// move a panel either.
    ///
    /// A night's new solves move a median by arcseconds, and the server
    /// refreshes the reading on every save, so a tighter test would call
    /// each night's solves a new layout. Activation keeps a shot target
    /// until it is a quarter of a panel away, 4.5′ for a 0.3° field, so half
    /// an arcminute is a tenth of the narrowest field and still far from a
    /// change it acts on. A 0.05° turn moves a 2° panel's corner by under
    /// 4″, and a thousandth of a side is a focus-scale drift.
    pub fn same_place(&self, other: &Self) -> bool {
        let apart = (self.position_angle_degrees - other.position_angle_degrees).rem_euclid(360.0);
        let side = |a: f64, b: f64| (a - b).abs() <= SAME_SIDE_SHARE * a.abs().max(b.abs());
        crate::ephemeris::separation_degrees(self.center, other.center) * 60.0 <= SAME_CENTER_ARCMIN
            && apart.min(360.0 - apart) <= SAME_ANGLE_DEGREES
            && side(self.size.width_degrees, other.size.width_degrees)
            && side(self.size.height_degrees, other.size.height_degrees)
    }

    /// The same geometry within what float drift and a JSON round trip leave.
    pub fn same(&self, other: &Self) -> bool {
        let close = |a: f64, b: f64| (a - b).abs() <= 1e-7;
        let apart = (self.position_angle_degrees - other.position_angle_degrees).rem_euclid(360.0);
        let ra_apart = (self.center.ra_degrees - other.center.ra_degrees).rem_euclid(360.0);
        ra_apart.min(360.0 - ra_apart) <= 1e-7
            && close(self.center.dec_degrees, other.center.dec_degrees)
            && apart.min(360.0 - apart) <= 1e-7
            && close(self.size.width_degrees, other.size.width_degrees)
            && close(self.size.height_degrees, other.size.height_degrees)
            && self.source == other.source
            && self.size_source == other.size_source
            && self.frames == other.frames
    }
}

/// What one frame says about where it was shot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameSample {
    pub center: IcrsPosition,
    /// Camera up, east of north at the frame's center, when known.
    pub position_angle_degrees: Option<f64>,
    /// The frame's field, when known.
    pub size: Option<PanelSize>,
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(|a, b| a.total_cmp(b));
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 1 {
        values[middle]
    } else {
        (values[middle - 1] + values[middle]) / 2.0
    })
}

/// An angle within a half turn either side of `reference`: a meridian flip
/// turns the field by exactly a half turn and covers the same sky.
fn fold(angle: f64, reference: f64) -> f64 {
    reference + (angle - reference + 90.0).rem_euclid(180.0) - 90.0
}

/// The axis the angles share, modulo a half turn: half the direction of
/// the mean of their doubled angles. `None` when they cancel out.
fn axial_mean(angles: &[f64]) -> Option<f64> {
    let (sin, cos) = angles.iter().fold((0.0, 0.0), |(s, c), angle| {
        let (sa, ca) = (2.0 * angle.to_radians()).sin_cos();
        (s + sa, c + ca)
    });
    (sin.hypot(cos) > 1e-9 * angles.len() as f64).then(|| sin.atan2(cos).to_degrees() / 2.0)
}

/// The mean direction of the samples' centers, as a sky position; `None`
/// when they cancel out.
fn mean_center(samples: &[FrameSample]) -> Option<IcrsPosition> {
    let mut sum = [0.0; 3];
    for sample in samples {
        let (sa, ca) = sample.center.ra_degrees.to_radians().sin_cos();
        let (sd, cd) = sample.center.dec_degrees.to_radians().sin_cos();
        sum = [sum[0] + cd * ca, sum[1] + cd * sa, sum[2] + sd];
    }
    let length = (sum[0] * sum[0] + sum[1] * sum[1] + sum[2] * sum[2]).sqrt();
    (length > 1e-9).then(|| IcrsPosition {
        ra_degrees: sum[1].atan2(sum[0]).to_degrees().rem_euclid(360.0) % 360.0,
        dec_degrees: (sum[2] / length).clamp(-1.0, 1.0).asin().to_degrees(),
    })
}

/// The median center, angle and size over `samples`.
///
/// Centers are taken on the plane at the samples' mean direction, so a
/// field across 0h is no trouble and one stray solve cannot pick the plane
/// and drop the rest. Angles are folded to within a quarter turn of the
/// axis they share (their axial mean), so frames either side of a meridian
/// flip agree even when the camera sits near ±90° from the plan; the
/// result is then given on the side of the half turn nearest `reference`
/// (the plan's angle). `None` without samples. The angle and the size are
/// `None` when no sample carries one.
pub fn median_geometry(
    samples: &[FrameSample],
    reference: f64,
) -> Option<(IcrsPosition, Option<f64>, Option<PanelSize>)> {
    let first = samples.first()?;
    let anchor = mean_center(samples).unwrap_or(first.center);
    let plane = TangentPlane::at(anchor).ok()?;
    let offsets: Vec<[f64; 2]> = samples
        .iter()
        .filter_map(|sample| plane.project(sample.center))
        .collect();
    let mut xs: Vec<f64> = offsets.iter().map(|o| o[0]).collect();
    let mut ys: Vec<f64> = offsets.iter().map(|o| o[1]).collect();
    let center = plane.deproject([median(&mut xs)?, median(&mut ys)?]);
    let raw: Vec<f64> = samples
        .iter()
        .filter_map(|sample| sample.position_angle_degrees)
        .filter(|angle| angle.is_finite())
        .collect();
    let axis = axial_mean(&raw).unwrap_or(reference);
    let mut angles: Vec<f64> = raw.iter().map(|angle| fold(*angle, axis)).collect();
    let angle = median(&mut angles).map(|angle| {
        let wrapped = fold(angle, reference).rem_euclid(360.0) % 360.0;
        if wrapped >= 360.0 {
            0.0
        } else {
            wrapped
        }
    });
    let mut widths: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.size.map(|size| size.width_degrees))
        .collect();
    let mut heights: Vec<f64> = samples
        .iter()
        .filter_map(|s| s.size.map(|size| size.height_degrees))
        .collect();
    let size = median(&mut widths)
        .zip(median(&mut heights))
        .map(|(width_degrees, height_degrees)| PanelSize {
            width_degrees,
            height_degrees,
        })
        .filter(|size| size.validate().is_ok());
    Some((center, angle, size))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(ra: f64, dec: f64, angle: Option<f64>) -> FrameSample {
        FrameSample {
            center: IcrsPosition {
                ra_degrees: ra,
                dec_degrees: dec,
            },
            position_angle_degrees: angle,
            size: Some(PanelSize {
                width_degrees: 1.8,
                height_degrees: 1.2,
            }),
        }
    }

    #[test]
    fn the_median_frame_stands_for_the_panel_and_a_stray_one_does_not_pull_it() {
        let samples = [
            sample(38.20, 61.45, Some(1.0)),
            sample(38.21, 61.46, Some(1.2)),
            sample(38.19, 61.44, Some(0.8)),
            // A frame centered far off, as one shot during a slew.
            sample(40.00, 62.50, Some(15.0)),
        ];
        let (center, angle, size) = median_geometry(&samples, 0.0).unwrap();
        assert!((center.ra_degrees - 38.205).abs() < 0.01, "{center:?}");
        assert!((center.dec_degrees - 61.455).abs() < 0.01, "{center:?}");
        assert!((angle.unwrap() - 1.1).abs() < 1e-9);
        assert_eq!(size.unwrap().width_degrees, 1.8);
    }

    #[test]
    fn frames_either_side_of_a_flip_agree_and_a_field_across_0h_stays_whole() {
        let samples = [
            sample(359.99, 10.0, Some(30.0)),
            sample(0.01, 10.0, Some(210.4)),
            sample(0.0, 10.0, Some(29.8)),
        ];
        let (center, angle, _) = median_geometry(&samples, 30.0).unwrap();
        assert!(
            center.ra_degrees < 0.02 || center.ra_degrees > 359.98,
            "{center:?}"
        );
        assert!((angle.unwrap() - 30.0).abs() < 1e-9, "{angle:?}");
        // A flip folds to the plan's side of the half turn.
        assert!((fold(210.4, 30.0) - 30.4).abs() < 1e-9);
        assert!((fold(-150.0, 30.0) - 30.0).abs() < 1e-9);
    }

    #[test]
    fn no_angle_and_no_samples_say_so() {
        assert!(median_geometry(&[], 0.0).is_none());
        let (_, angle, _) = median_geometry(&[sample(10.0, 20.0, None)], 0.0).unwrap();
        assert!(angle.is_none());
    }
}
