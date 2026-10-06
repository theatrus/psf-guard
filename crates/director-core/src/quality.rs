//! Conservative local screening, not image grading or proof of cloud cover.
//! A caller supplies a frozen, explicitly approved reference in physical ADU.
//! The classifier never learns from a deteriorating stream or issues hardware work.

use crate::{recovery::Verdict, valid_id};
use serde::{Deserialize, Serialize};

pub const VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Context {
    pub rig_id: String,
    pub configuration_id: String,
    pub target_id: String,
    /// Binds filter, exposure, binning, gain, offset and readout mode together.
    pub recipe_fingerprint: String,
    /// Includes detector implementation, settings and measurement units.
    pub analysis_fingerprint: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Metrics {
    pub stars: Option<u32>,
    pub hfr_pixels: Option<f64>,
    pub background_adu: Option<f64>,
    pub eccentricity: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub capture_id: String,
    pub observed_at_ms: u64,
    pub context: Context,
    pub metrics: Metrics,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub id: String,
    pub approved: bool,
    pub frame: Frame,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Policy {
    pub minimum_reference_stars: u32,
    pub poor_star_ratio: f64,
    pub good_star_ratio: f64,
    pub poor_background_ratio: f64,
    pub good_background_ratio: f64,
    pub maximum_hfr_ratio: f64,
    pub maximum_eccentricity: f64,
    pub evidence_max_age_ms: u64,
    pub reference_max_age_ms: u64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            minimum_reference_stars: 20,
            poor_star_ratio: 0.5,
            good_star_ratio: 0.85,
            poor_background_ratio: 1.5,
            good_background_ratio: 1.2,
            maximum_hfr_ratio: 1.3,
            maximum_eccentricity: 0.65,
            evidence_max_age_ms: 30_000,
            reference_max_age_ms: 6 * 3_600_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    ReferenceUnapproved,
    IncompatibleContext,
    StaleEvidence,
    ReferenceExpired,
    SameCapture,
    MissingMetrics,
    InsufficientReference,
    FocusOrTracking,
    Ambiguous,
    StarLossAndBackgroundRise,
    ConsistentWithReference,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Assessment {
    pub verdict: Verdict,
    pub reason: Reason,
    pub star_ratio: Option<f64>,
    pub background_ratio: Option<f64>,
    pub hfr_ratio: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPolicy,
    InvalidFrame,
}

impl Policy {
    pub fn validate(&self) -> Result<(), Error> {
        if !(10..=1_000_000).contains(&self.minimum_reference_stars)
            || !self.poor_star_ratio.is_finite()
            || !(0.0..1.0).contains(&self.poor_star_ratio)
            || !self.good_star_ratio.is_finite()
            || self.good_star_ratio <= self.poor_star_ratio
            || self.good_star_ratio > 1.0
            || !self.poor_background_ratio.is_finite()
            || self.poor_background_ratio <= 1.0
            || self.poor_background_ratio > 10.0
            || !self.good_background_ratio.is_finite()
            || self.good_background_ratio < 1.0
            || self.good_background_ratio >= self.poor_background_ratio
            || !self.maximum_hfr_ratio.is_finite()
            || !(1.0..=2.0).contains(&self.maximum_hfr_ratio)
            || !self.maximum_eccentricity.is_finite()
            || !(0.0..1.0).contains(&self.maximum_eccentricity)
            || self.evidence_max_age_ms == 0
            || self.evidence_max_age_ms > 300_000
            || self.reference_max_age_ms < self.evidence_max_age_ms
            || self.reference_max_age_ms > 86_400_000
        {
            return Err(Error::InvalidPolicy);
        }
        Ok(())
    }
}

impl Frame {
    fn validate(&self) -> Result<(), Error> {
        let c = &self.context;
        let m = &self.metrics;
        if ![
            &self.capture_id,
            &c.rig_id,
            &c.configuration_id,
            &c.target_id,
            &c.recipe_fingerprint,
            &c.analysis_fingerprint,
        ]
        .into_iter()
        .all(|id| valid_id(id))
            || c.width == 0
            || c.height == 0
            || c.width > 100_000
            || c.height > 100_000
            || self.observed_at_ms > i64::MAX as u64
            || m.stars.is_some_and(|n| n > 10_000_000)
            || m.hfr_pixels.is_some_and(|x| !x.is_finite() || x <= 0.0)
            || m.background_adu.is_some_and(|x| !x.is_finite() || x <= 0.0)
            || m.eccentricity
                .is_some_and(|x| !x.is_finite() || !(0.0..1.0).contains(&x))
        {
            return Err(Error::InvalidFrame);
        }
        Ok(())
    }
}

pub fn classify(
    policy: &Policy,
    reference: &Reference,
    frame: &Frame,
    now_ms: u64,
) -> Result<Assessment, Error> {
    policy.validate()?;
    reference.frame.validate()?;
    frame.validate()?;
    if !valid_id(&reference.id) || now_ms > i64::MAX as u64 {
        return Err(Error::InvalidFrame);
    }
    let unknown = |reason| Assessment {
        verdict: Verdict::Unknown,
        reason,
        star_ratio: None,
        background_ratio: None,
        hfr_ratio: None,
    };
    if !reference.approved {
        return Ok(unknown(Reason::ReferenceUnapproved));
    }
    if reference.frame.context != frame.context {
        return Ok(unknown(Reason::IncompatibleContext));
    }
    if reference.frame.capture_id == frame.capture_id {
        return Ok(unknown(Reason::SameCapture));
    }
    if frame.observed_at_ms > now_ms
        || now_ms - frame.observed_at_ms > policy.evidence_max_age_ms
        || frame.observed_at_ms <= reference.frame.observed_at_ms
    {
        return Ok(unknown(Reason::StaleEvidence));
    }
    if now_ms - reference.frame.observed_at_ms > policy.reference_max_age_ms {
        return Ok(unknown(Reason::ReferenceExpired));
    }
    let (r, m) = (&reference.frame.metrics, &frame.metrics);
    let (Some(rs), Some(rh), Some(rb), Some(re), Some(s), Some(h), Some(b), Some(e)) = (
        r.stars,
        r.hfr_pixels,
        r.background_adu,
        r.eccentricity,
        m.stars,
        m.hfr_pixels,
        m.background_adu,
        m.eccentricity,
    ) else {
        return Ok(unknown(Reason::MissingMetrics));
    };
    if rs < policy.minimum_reference_stars || re > policy.maximum_eccentricity {
        return Ok(unknown(Reason::InsufficientReference));
    }
    let (stars, background, hfr) = (f64::from(s) / f64::from(rs), b / rb, h / rh);
    if ![stars, background, hfr].into_iter().all(f64::is_finite) {
        return Err(Error::InvalidFrame);
    }
    let (verdict, reason) = if hfr > policy.maximum_hfr_ratio || e > policy.maximum_eccentricity {
        (Verdict::Unknown, Reason::FocusOrTracking)
    } else if stars <= policy.poor_star_ratio && background >= policy.poor_background_ratio {
        (Verdict::CorroboratedPoor, Reason::StarLossAndBackgroundRise)
    } else if stars >= policy.good_star_ratio && background <= policy.good_background_ratio {
        (Verdict::ConfirmedGood, Reason::ConsistentWithReference)
    } else {
        (Verdict::Unknown, Reason::Ambiguous)
    };
    Ok(Assessment {
        verdict,
        reason,
        star_ratio: Some(stars),
        background_ratio: Some(background),
        hfr_ratio: Some(hfr),
    })
}
