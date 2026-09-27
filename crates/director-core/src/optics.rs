//! Rig optical geometry for framing previews. Pure arithmetic on declared
//! sensor and telescope values; it never reads FITS files, equipment or a
//! clock, and a computed field of view is not pointing evidence.

use serde::{Deserialize, Serialize};

/// Arcseconds subtended by one micrometre at one millimetre focal length.
const ARCSEC_PER_RADIAN: f64 = 206_264.806_247_096_36;

/// How the camera angle can change between the plan and the sky.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum Rotation {
    /// The camera is bolted at one angle; framing must accept it.
    Fixed { angle_degrees: f64 },
    /// An operator can turn the camera before a session, not during one.
    Manual { angle_degrees: f64 },
    /// A rotator reaches any requested position angle unattended.
    Rotator {},
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Optics {
    /// Unbinned sensor size in pixels.
    pub sensor_width_px: u32,
    pub sensor_height_px: u32,
    /// Unbinned pixel pitch in micrometres.
    pub pixel_size_um: f64,
    /// Effective focal length, after any reducer or extender.
    pub focal_length_mm: f64,
    /// Clear aperture. Optional because some header sets never state it.
    #[serde(deserialize_with = "Option::deserialize")]
    pub aperture_mm: Option<f64>,
    pub rotation: Rotation,
}

/// Values every framing view needs, computed one way for server and browser.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FieldOfView {
    pub width_degrees: f64,
    pub height_degrees: f64,
    pub pixel_scale_arcsec: f64,
    #[serde(deserialize_with = "Option::deserialize")]
    pub focal_ratio: Option<f64>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpticsError {
    InvalidSensor,
    InvalidPixelSize,
    InvalidFocalLength,
    InvalidAperture,
    InvalidRotation,
}

fn bounded(value: f64, minimum: f64, maximum: f64) -> bool {
    value.is_finite() && (minimum..=maximum).contains(&value)
}

impl Rotation {
    pub fn validate(&self) -> Result<(), OpticsError> {
        match self {
            Self::Fixed { angle_degrees } | Self::Manual { angle_degrees } => {
                if !angle_degrees.is_finite() || !(0.0..360.0).contains(angle_degrees) {
                    return Err(OpticsError::InvalidRotation);
                }
            }
            Self::Rotator {} => {}
        }
        Ok(())
    }
}

impl Optics {
    /// Bounds cover any amateur or professional instrument that can plausibly
    /// feed this planner; anything outside is a data error, not a real rig.
    pub fn validate(&self) -> Result<(), OpticsError> {
        if !(1..=65_536).contains(&self.sensor_width_px)
            || !(1..=65_536).contains(&self.sensor_height_px)
        {
            return Err(OpticsError::InvalidSensor);
        }
        if !bounded(self.pixel_size_um, 0.5, 100.0) {
            return Err(OpticsError::InvalidPixelSize);
        }
        if !bounded(self.focal_length_mm, 10.0, 20_000.0) {
            return Err(OpticsError::InvalidFocalLength);
        }
        if self
            .aperture_mm
            .is_some_and(|aperture| !bounded(aperture, 5.0, 10_000.0))
        {
            return Err(OpticsError::InvalidAperture);
        }
        self.rotation.validate()
    }

    /// Small-angle field of view. Valid for every field a camera sensor sees;
    /// the error at ten degrees of half-width is under one part in a thousand.
    pub fn field_of_view(&self) -> Result<FieldOfView, OpticsError> {
        self.validate()?;
        let pixel_scale_arcsec =
            ARCSEC_PER_RADIAN * self.pixel_size_um / 1000.0 / self.focal_length_mm;
        Ok(FieldOfView {
            width_degrees: f64::from(self.sensor_width_px) * pixel_scale_arcsec / 3600.0,
            height_degrees: f64::from(self.sensor_height_px) * pixel_scale_arcsec / 3600.0,
            pixel_scale_arcsec,
            focal_ratio: self
                .aperture_mm
                .map(|aperture| self.focal_length_mm / aperture),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redcat() -> Optics {
        Optics {
            sensor_width_px: 6248,
            sensor_height_px: 4176,
            pixel_size_um: 3.76,
            focal_length_mm: 250.0,
            aperture_mm: Some(51.0),
            rotation: Rotation::Manual {
                angle_degrees: 12.5,
            },
        }
    }

    #[test]
    fn a_known_refractor_and_camera_give_the_published_scale_and_field() {
        let fov = redcat().field_of_view().unwrap();
        assert!((fov.pixel_scale_arcsec - 3.1022).abs() < 0.001, "{fov:?}");
        assert!((fov.width_degrees - 5.384).abs() < 0.002, "{fov:?}");
        assert!((fov.height_degrees - 3.599).abs() < 0.002, "{fov:?}");
        assert!((fov.focal_ratio.unwrap() - 4.9).abs() < 0.01);
    }

    #[test]
    fn out_of_range_values_are_named_not_clamped() {
        let mut optics = redcat();
        optics.sensor_width_px = 0;
        assert_eq!(optics.validate(), Err(OpticsError::InvalidSensor));
        let mut optics = redcat();
        optics.pixel_size_um = f64::NAN;
        assert_eq!(optics.validate(), Err(OpticsError::InvalidPixelSize));
        let mut optics = redcat();
        optics.focal_length_mm = 5.0;
        assert_eq!(optics.validate(), Err(OpticsError::InvalidFocalLength));
        let mut optics = redcat();
        optics.aperture_mm = Some(0.0);
        assert_eq!(optics.validate(), Err(OpticsError::InvalidAperture));
        let mut optics = redcat();
        optics.rotation = Rotation::Fixed {
            angle_degrees: 360.0,
        };
        assert_eq!(optics.validate(), Err(OpticsError::InvalidRotation));
        optics.rotation = Rotation::Rotator {};
        assert!(optics.validate().is_ok());
        optics.aperture_mm = None;
        assert_eq!(optics.field_of_view().unwrap().focal_ratio, None);
    }

    #[test]
    fn json_is_strict_and_round_trips() {
        let json = serde_json::to_string(&redcat()).unwrap();
        assert!(json.contains("\"mode\":\"manual\""));
        let back: Optics = serde_json::from_str(&json).unwrap();
        assert_eq!(back, redcat());
        assert!(
            serde_json::from_str::<Optics>(&json.replace("\"aperture_mm\":51.0,", "")).is_err()
        );
        assert!(serde_json::from_str::<Optics>(&json.replace("}}", ",\"extra\":1}}")).is_err());
    }
}
