//! Filter Moon avoidance policy shared by planning and local execution.
//! The TS-compatible Lorentzian uses days from full Moon and degrees. This
//! module accepts evidence explicitly; it does not read clocks or devices.

use serde::{Deserialize, Serialize};
mod windows;
pub use windows::moon_windows;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoonPolicy {
    pub enabled: bool,
    pub separation_degrees: f64,
    /// Days either side of full Moon at which the required separation halves.
    pub width_days: f64,
    pub relax_degrees_per_degree: f64,
    pub relax_min_altitude_degrees: f64,
    pub relax_max_altitude_degrees: f64,
    /// TS semantics: block at/above relax_max, not an implicit zero horizon.
    pub moon_down: bool,
}

impl Default for MoonPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            separation_degrees: 60.0,
            width_days: 7.0,
            relax_degrees_per_degree: 0.0,
            relax_min_altitude_degrees: -15.0,
            relax_max_altitude_degrees: 5.0,
            moon_down: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoonEvidence {
    /// Absolute phase distance from full Moon, in the TS 29.5-day convention.
    pub days_from_full: f64,
    pub altitude_degrees: f64,
    pub separation_degrees: f64,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MoonReason {
    Disabled,
    Allowed,
    RelaxedBelowMinimum,
    RelaxedSeparation,
    MoonMustBeDown,
    TooClose,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MoonDecision {
    pub allowed: bool,
    pub reason: MoonReason,
    pub required_separation_degrees: f64,
    /// Static TS aversion: rank eligible filters, never override a rejection.
    pub aversion: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MoonError {
    InvalidPolicy,
    InvalidEvidence,
}

fn bounded(value: f64, low: f64, high: f64) -> bool {
    value.is_finite() && (low..=high).contains(&value)
}

impl MoonPolicy {
    pub fn validate(&self) -> Result<(), MoonError> {
        if !bounded(self.separation_degrees, 0.0, 180.0)
            || !bounded(self.width_days, 1.0, 14.0)
            || self.width_days.fract() != 0.0
            || !bounded(self.relax_degrees_per_degree, 0.0, 180.0)
            || !bounded(self.relax_min_altitude_degrees, -90.0, 90.0)
            || !bounded(self.relax_max_altitude_degrees, -90.0, 90.0)
            || self.relax_min_altitude_degrees >= self.relax_max_altitude_degrees
        {
            return Err(MoonError::InvalidPolicy);
        }
        Ok(())
    }

    pub fn aversion(&self) -> Result<f64, MoonError> {
        self.validate()?;
        Ok(if !self.enabled {
            0.0
        } else if self.moon_down {
            1.0
        } else {
            (self.separation_degrees * self.width_days / (180.0 * 14.0)).abs()
        })
    }

    pub fn evaluate(&self, evidence: MoonEvidence) -> Result<MoonDecision, MoonError> {
        self.validate()?;
        let aversion = self.aversion()?;
        let result = |allowed, reason, required_separation_degrees| MoonDecision {
            allowed,
            reason,
            required_separation_degrees,
            aversion,
        };
        if !self.enabled {
            return Ok(result(true, MoonReason::Disabled, 0.0));
        }
        if !bounded(evidence.days_from_full, 0.0, 14.75)
            || !bounded(evidence.altitude_degrees, -90.0, 90.0)
            || !bounded(evidence.separation_degrees, 0.0, 180.0)
        {
            return Err(MoonError::InvalidEvidence);
        }
        let relaxing = self.relax_degrees_per_degree > 0.0;
        if relaxing && evidence.altitude_degrees <= self.relax_min_altitude_degrees {
            return Ok(result(true, MoonReason::RelaxedBelowMinimum, 0.0));
        }
        if self.moon_down && evidence.altitude_degrees >= self.relax_max_altitude_degrees {
            return Ok(result(false, MoonReason::MoonMustBeDown, 0.0));
        }
        let mut separation = self.separation_degrees;
        let mut width = self.width_days;
        if relaxing && evidence.altitude_degrees <= self.relax_max_altitude_degrees {
            separation += self.relax_degrees_per_degree
                * (evidence.altitude_degrees - self.relax_max_altitude_degrees);
            width *= (evidence.altitude_degrees - self.relax_min_altitude_degrees)
                / (self.relax_max_altitude_degrees - self.relax_min_altitude_degrees);
        }
        if separation <= 0.0 {
            return Ok(result(true, MoonReason::RelaxedSeparation, 0.0));
        }
        // At the lower boundary we returned above, so width remains positive.
        let required = separation / (1.0 + (evidence.days_from_full / width).powi(2));
        let allowed = evidence.separation_degrees >= required;
        Ok(result(
            allowed,
            if allowed {
                MoonReason::Allowed
            } else {
                MoonReason::TooClose
            },
            required,
        ))
    }
}
