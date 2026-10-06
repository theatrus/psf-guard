//! Conservative local screening, not image grading or proof of cloud cover.
//! A frozen reference is approved or built from a stable initial cohort in ADU.
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
    /// Empty for an operator-approved individual frame. A provisional baseline
    /// retains its original cohort so its stability can be verified on readback.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub initial_group: Vec<Frame>,
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
    pub reference_quality_unknown: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPolicy,
    InvalidFrame,
    InsufficientSamples,
    UnstableBaseline,
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
        reference_quality_unknown: !reference.approved,
    };
    let mut baseline = reference.frame.clone();
    if !reference.initial_group.is_empty() {
        let built = build_initial_reference(policy, &reference.id, &reference.initial_group)?;
        if built.frame != reference.frame {
            return Err(Error::InvalidFrame);
        }
        baseline.metrics = group_metrics(&reference.initial_group);
    } else if !reference.approved {
        return Ok(unknown(Reason::ReferenceUnapproved));
    }
    if reference.frame.context != frame.context {
        return Ok(unknown(Reason::IncompatibleContext));
    }
    if reference.frame.capture_id == frame.capture_id
        || reference
            .initial_group
            .iter()
            .any(|f| f.capture_id == frame.capture_id)
    {
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
    let (r, m) = (&baseline.metrics, &frame.metrics);
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
        reference_quality_unknown: !reference.approved,
    })
}

/// Freeze the first stable cohort. The host must persist this result and never
/// replace it automatically as conditions deteriorate. Stability is not quality.
pub fn build_initial_reference(
    policy: &Policy,
    id: &str,
    frames: &[Frame],
) -> Result<Reference, Error> {
    policy.validate()?;
    if !valid_id(id) || frames.len() > 16 {
        return Err(Error::InvalidFrame);
    }
    if frames.len() < 5 {
        return Err(Error::InsufficientSamples);
    }
    let first = &frames[0];
    let mut ids = std::collections::BTreeSet::new();
    for (i, frame) in frames.iter().enumerate() {
        frame.validate()?;
        if !ids.insert(&frame.capture_id)
            || frame.context != first.context
            || i > 0 && frame.observed_at_ms <= frames[i - 1].observed_at_ms
        {
            return Err(Error::InvalidFrame);
        }
        if frame.observed_at_ms - first.observed_at_ms > 7_200_000 {
            return Err(Error::UnstableBaseline);
        }
        let m = &frame.metrics;
        if m.stars.is_none_or(|n| n < policy.minimum_reference_stars)
            || m.hfr_pixels.is_none()
            || m.background_adu.is_none()
            || m.eccentricity
                .is_none_or(|e| e > policy.maximum_eccentricity)
        {
            return Err(Error::UnstableBaseline);
        }
    }
    let spread = |values: Vec<f64>, limit: f64| {
        let min = values.iter().copied().fold(f64::INFINITY, f64::min);
        let max = values.iter().copied().fold(0.0, f64::max);
        max / min <= limit
    };
    if !spread(
        frames
            .iter()
            .map(|f| f64::from(f.metrics.stars.unwrap()))
            .collect(),
        1.25,
    ) || !spread(
        frames
            .iter()
            .map(|f| f.metrics.hfr_pixels.unwrap())
            .collect(),
        1.15,
    ) || !spread(
        frames
            .iter()
            .map(|f| f.metrics.background_adu.unwrap())
            .collect(),
        1.2,
    ) {
        return Err(Error::UnstableBaseline);
    }
    Ok(Reference {
        id: id.into(),
        approved: false,
        frame: frames.last().unwrap().clone(),
        initial_group: frames.to_vec(),
    })
}

fn group_metrics(frames: &[Frame]) -> Metrics {
    fn median(mut values: Vec<f64>) -> f64 {
        values.sort_by(f64::total_cmp);
        let n = values.len();
        if n.is_multiple_of(2) {
            values[n / 2 - 1] / 2.0 + values[n / 2] / 2.0
        } else {
            values[n / 2]
        }
    }
    Metrics {
        stars: Some(
            median(
                frames
                    .iter()
                    .map(|f| f64::from(f.metrics.stars.unwrap()))
                    .collect(),
            )
            .round() as u32,
        ),
        hfr_pixels: Some(median(
            frames
                .iter()
                .map(|f| f.metrics.hfr_pixels.unwrap())
                .collect(),
        )),
        background_adu: Some(median(
            frames
                .iter()
                .map(|f| f.metrics.background_adu.unwrap())
                .collect(),
        )),
        eccentricity: Some(median(
            frames
                .iter()
                .map(|f| f.metrics.eccentricity.unwrap())
                .collect(),
        )),
    }
}
