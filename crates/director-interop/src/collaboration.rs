//! Reviewed import and contribution snapshots. Hosts supply authenticated
//! payloads and verified image evidence; neither operation grants acquisition.

use crate::{astrocollab::*, json as bounded, Error};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const MAX_REPORTS: usize = 200;
pub const MAX_REPORT_FRAMES: usize = 4096;
pub const MAX_PRESENCE_AGE_MS: u64 = 60_000;

#[derive(Clone, Debug, Serialize)]
pub struct PreparedImport {
    source: Source,
    night: String,
    share: Share,
    import_id: Uuid,
    project_id: Uuid,
    digest: String,
    #[serde(skip)]
    wire: Vec<u8>,
}

impl PreparedImport {
    pub fn source(&self) -> &Source {
        &self.source
    }
    pub fn night(&self) -> &str {
        &self.night
    }
    pub fn share(&self) -> &Share {
        &self.share
    }
    pub fn import_id(&self) -> Uuid {
        self.import_id
    }
    pub fn project_id(&self) -> Uuid {
        self.project_id
    }
    pub fn digest(&self) -> &str {
        &self.digest
    }
    /// Canonical public-wire snapshot, stripped of unknown fields and secrets.
    /// Re-read through `prepare_import`; never trust stored normalized structs.
    pub fn snapshot(&self) -> &[u8] {
        &self.wire
    }
}

pub fn digest(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    Sha256::digest(bytes)
        .iter()
        .flat_map(|b| {
            [
                HEX[(b >> 4) as usize] as char,
                HEX[(b & 15) as usize] as char,
            ]
        })
        .collect()
}

pub fn prepare_import(
    bytes: &[u8],
    source: &Source,
    night: &str,
    task_id: &str,
) -> Result<PreparedImport, Error> {
    let nightly = decode_tonight(bytes, source, night)?;
    let share = nightly
        .shares
        .into_iter()
        .find(|s| s.task_id == task_id)
        .ok_or(Error::InvalidIdentity)?;
    if !share.review_reasons.is_empty() || share.demands.is_empty() {
        return Err(Error::NeedsReview);
    }
    let wire = canonical_snapshot(source, &share)?;
    let key = serde_json::to_vec(&(source.base_url(), source.agent_id(), &share.task_id, night))
        .map_err(|_| Error::InvalidValue)?;
    let import_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, &key);
    let key = serde_json::to_vec(&(source.base_url(), &share.project_id))
        .map_err(|_| Error::InvalidValue)?;
    let project_id = Uuid::new_v5(&Uuid::NAMESPACE_URL, &key);
    let key = serde_json::to_vec(&(source, night, &wire)).map_err(|_| Error::InvalidValue)?;
    Ok(PreparedImport {
        source: source.clone(),
        night: night.into(),
        share,
        import_id,
        project_id,
        digest: digest(&key),
        wire,
    })
}

fn region_wire(region: &Region) -> Value {
    const MAS: f64 = psf_guard_director_core::program::MAS_PER_DEGREE as f64;
    json!({
        "ra": f64::from(region.icrs_ra_mas) / MAS,
        "dec": f64::from(region.icrs_dec_mas) / MAS,
        "width": f64::from(region.width_mas) / MAS,
        "height": f64::from(region.height_mas) / MAS,
        "rotation": f64::from(region.position_angle_mas) / MAS,
    })
}

fn canonical_snapshot(source: &Source, s: &Share) -> Result<Vec<u8>, Error> {
    let r = s.requirements.as_ref().ok_or(Error::NeedsReview)?;
    let cells: Vec<_> = s
        .cells
        .iter()
        .map(|c| {
            let mut v = region_wire(&c.region);
            v["row"] = json!(c.row);
            v["column"] = json!(c.column);
            v
        })
        .collect();
    let mut frames = serde_json::Map::new();
    let mut exposures = std::collections::BTreeMap::new();
    for d in &s.demands {
        frames.insert(d.filter.clone(), json!(d.requested_frames));
        exposures.insert(d.filter.clone(), d.exposure_ms);
    }
    let filters: Vec<_> = exposures
        .into_iter()
        .map(|(filter, exposure)| json!({"filter": filter, "exposure": exposure as f64 / 1000.0}))
        .collect();
    let requirements = json!({
        "minFocalLength": r.min_focal_length_mm, "maxFocalLength": r.max_focal_length_mm,
        "minScale": r.min_scale_arcsec, "maxScale": r.max_scale_arcsec,
        "acceptColour": r.accept_colour, "colourMaxMoon": r.colour_max_moon,
        "maxHfr": r.max_hfr_arcsec, "maxGuideRms": r.max_guide_rms_arcsec,
        "minExposure": r.min_exposure_ms.map(|n| n as f64 / 1000.0),
        "maxExposure": r.max_exposure_ms.map(|n| n as f64 / 1000.0),
        "filters": r.filters_nm, "maxMoonIllumination": r.max_moon_illumination,
        "minMoonSeparation": r.min_moon_separation_degrees, "minAltitude": r.min_altitude_degrees,
        "requireCalibrated": r.require_calibrated, "minFramesPerVisit": r.min_frames_per_visit,
    });
    let mut task = json!({
        "id": s.task_id, "project": s.project_id, "agent": source.agent_id(),
        "version": s.version, "state": "accepted", "kind": s.kind,
        "region": region_wire(&s.region), "cells": cells, "share": s.panel_order,
        "assignedNight": s.assigned_night, "filters": filters, "visit": {"frames": frames},
    });
    if let Some(name) = &s.name {
        task["projectName"] = json!(name);
    }
    let body = json!({"protocol": WIRE_PROTOCOL, "tasks": [task], "requirements": requirements});
    let bytes = serde_json::to_vec(&body).map_err(|_| Error::InvalidValue)?;
    // Canonicalizing must not silently exceed the decoder's structural limits.
    decode_tonight(
        &bytes,
        source,
        s.assigned_night.as_deref().ok_or(Error::NeedsReview)?,
    )?;
    Ok(bytes)
}

/// Evidence already checked against a saved catalog image by the host.
/// Fingerprints bind the pixel solve to that image, not its planned center.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameEvidence {
    pub capture_id: Uuid,
    pub image_guid: Uuid,
    pub source_digest: String,
    pub panel_index: u32,
    pub filter: String,
    pub exposure_ms: u64,
    pub saved: bool,
    pub accepted: bool,
    pub finalized: bool,
    pub image_fingerprint: String,
    pub solve_fingerprint: String,
    pub solved_footprint: MeasuredFootprint,
    pub scale_arcsec: Option<f64>,
    pub focal_length_mm: Option<f64>,
    pub hfr_arcsec: Option<f64>,
    pub guide_rms_arcsec: Option<f64>,
    pub moon_illumination: Option<f64>,
    pub moon_separation_degrees: Option<f64>,
    pub calibrated: bool,
    pub bandpass_nm: Option<f64>,
    pub colour: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MeasuredFootprint {
    pub ra: f64,
    pub dec: f64,
    pub width: f64,
    pub height: f64,
    pub rotation: f64,
}

impl MeasuredFootprint {
    fn validate(&self) -> Result<(), Error> {
        if !(finite(self.ra, 0.0, 360.0)
            && self.ra < 360.0
            && finite(self.dec, -90.0, 90.0)
            && finite(self.width, 0.0, 30.0)
            && self.width > 0.0
            && finite(self.height, 0.0, 30.0)
            && self.height > 0.0
            && finite(self.rotation, 0.0, 360.0)
            && self.rotation < 360.0)
        {
            return Err(Error::InvalidEvidence);
        }
        Ok(())
    }
}

fn finite(v: f64, low: f64, high: f64) -> bool {
    v.is_finite() && (low..=high).contains(&v)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ContributionReport {
    project: String,
    task: String,
    night: String,
    filter_name: String,
    panel: String,
    frames: u32,
    seconds: f64,
    exposure: f64,
    footprint: MeasuredFootprint,
    scale: Option<f64>,
    focal_length: Option<f64>,
    hfr: Option<f64>,
    guide_rms: Option<f64>,
    moon_illumination: Option<f64>,
    moon_separation: Option<f64>,
    calibrated: bool,
    bandpass: Option<f64>,
    colour: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct FinalizedContribution {
    import_id: Uuid,
    source_digest: String,
    geometry_digest: String,
    panel_index: u32,
    filter: String,
    integration_ms: u64,
    captures: Vec<Uuid>,
    images: Vec<Uuid>,
    report: ContributionReport,
}

impl FinalizedContribution {
    pub fn observing_night(&self) -> &str {
        &self.report.night
    }
    pub fn import_id(&self) -> Uuid {
        self.import_id
    }
    pub fn source_digest(&self) -> &str {
        &self.source_digest
    }
    pub fn geometry_digest(&self) -> &str {
        &self.geometry_digest
    }
    pub fn panel_index(&self) -> u32 {
        self.panel_index
    }
    pub fn filter(&self) -> &str {
        &self.filter
    }
    pub fn integration_ms(&self) -> u64 {
        self.integration_ms
    }
    pub fn captures(&self) -> &[Uuid] {
        &self.captures
    }
    pub fn images(&self) -> &[Uuid] {
        &self.images
    }
    pub fn report(&self) -> &ContributionReport {
        &self.report
    }
}

/// Finalize one homogeneous panel/filter/exposure cohort. A host must derive
/// the evidence from its catalog; this is not a public grading ingress API.
pub fn finalize_contribution(
    import: &PreparedImport,
    frames: &[FrameEvidence],
) -> Result<FinalizedContribution, Error> {
    finalize_contribution_for_night(import, frames, import.night())
}

/// An assignment is not a deadline. Keep its identity but report the actual
/// observing night, including captures acquired after it was first dealt.
pub fn finalize_contribution_for_night(
    import: &PreparedImport,
    frames: &[FrameEvidence],
    observing_night: &str,
) -> Result<FinalizedContribution, Error> {
    validate_night(observing_night)?;
    let first = frames.first().ok_or(Error::InvalidEvidence)?;
    if frames.len() > MAX_REPORT_FRAMES {
        return Err(Error::LimitExceeded);
    }
    let filter = fold_filter(&first.filter)?;
    let demand = import
        .share
        .demands
        .iter()
        .find(|d| d.panel_index == first.panel_index && d.filter == filter)
        .ok_or(Error::InvalidEvidence)?;
    let mut captures = BTreeMap::new();
    let mut images = BTreeSet::new();
    let mut integration_ms = 0_u64;
    for f in frames {
        f.solved_footprint.validate()?;
        if f.capture_id.is_nil()
            || f.image_guid.is_nil()
            || captures.insert(f.capture_id, f.image_guid).is_some()
            || !images.insert(f.image_guid)
            || f.source_digest != import.digest
            || f.panel_index != first.panel_index
            || fold_filter(&f.filter)? != filter
            || f.exposure_ms != demand.exposure_ms
            || !f.saved
            || !f.accepted
            || !f.finalized
            || f.image_fingerprint.is_empty()
            || f.image_fingerprint.len() > 256
            || f.image_fingerprint.chars().any(char::is_control)
            || f.image_fingerprint != f.solve_fingerprint
            || f.solved_footprint != first.solved_footprint
            || f.bandpass_nm != first.bandpass_nm
            || f.colour != first.colour
        {
            return Err(Error::InvalidEvidence);
        }
        for (v, low, high) in [
            (f.scale_arcsec, 0.0, 1e6),
            (f.focal_length_mm, 0.0, 1e6),
            (f.hfr_arcsec, 0.0, 1e6),
            (f.guide_rms_arcsec, 0.0, 1e6),
            (f.moon_illumination, 0.0, 1.0),
            (f.moon_separation_degrees, 0.0, 180.0),
            (f.bandpass_nm, 0.0, 1e6),
        ] {
            if v.is_some_and(|n| !finite(n, low, high)) {
                return Err(Error::InvalidEvidence);
            }
        }
        if f.scale_arcsec == Some(0.0)
            || f.focal_length_mm == Some(0.0)
            || f.bandpass_nm == Some(0.0)
        {
            return Err(Error::InvalidEvidence);
        }
        integration_ms = integration_ms
            .checked_add(f.exposure_ms)
            .ok_or(Error::InvalidEvidence)?;
        if integration_ms > 9_007_199_254_740_991 {
            return Err(Error::InvalidEvidence);
        }
    }
    // Missing measurements make the aggregate unknown, not a mean over just
    // the better-instrumented frames. Do not manufacture planned evidence.
    let mean = |get: fn(&FrameEvidence) -> Option<f64>| -> Option<f64> {
        let values: Option<Vec<_>> = frames.iter().map(get).collect();
        values.map(|v| v.iter().sum::<f64>() / v.len() as f64)
    };
    let calibrated = frames.iter().all(|f| f.calibrated);
    if import
        .share
        .requirements
        .as_ref()
        .is_some_and(|r| r.require_calibrated)
        && !calibrated
    {
        return Err(Error::NeedsReview);
    }
    let report = ContributionReport {
        project: import.share.project_id.clone(),
        task: import.share.task_id.clone(),
        night: observing_night.into(),
        filter_name: filter.clone(),
        panel: first.panel_index.to_string(),
        frames: frames.len() as u32,
        seconds: integration_ms as f64 / 1000.0,
        exposure: demand.exposure_ms as f64 / 1000.0,
        footprint: first.solved_footprint.clone(),
        scale: mean(|f| f.scale_arcsec),
        focal_length: mean(|f| f.focal_length_mm),
        hfr: mean(|f| f.hfr_arcsec),
        guide_rms: mean(|f| f.guide_rms_arcsec),
        moon_illumination: mean(|f| f.moon_illumination),
        moon_separation: mean(|f| f.moon_separation_degrees),
        calibrated,
        bandpass: first.bandpass_nm,
        colour: first.colour,
    };
    Ok(FinalizedContribution {
        import_id: import.import_id,
        source_digest: import.digest.clone(),
        geometry_digest: import.share.geometry_digest.clone(),
        panel_index: first.panel_index,
        filter,
        integration_ms,
        captures: captures.keys().copied().collect(),
        images: captures.values().copied().collect(),
        report,
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Recorded {
    pub id: String,
    pub accepted: bool,
    pub duplicate: bool,
    pub verdict: Verdict,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct Verdict {
    pub accepted: bool,
    pub reasons: Vec<String>,
    pub unverified: Vec<String>,
    pub summary: Option<String>,
    pub overridden_by: Option<String>,
    pub override_reason: Option<String>,
}

/// Public replies may add fields; validation consumes only the known ones.
/// A short/malformed batch acknowledges nothing, even after a partial commit.
pub fn decode_recorded(bytes: &[u8], expected: usize) -> Result<Vec<Recorded>, Error> {
    if expected == 0 || expected > MAX_REPORTS {
        return Err(Error::InvalidReply);
    }
    let v = bounded::decode(bytes)?;
    let map = bounded::object(&v)?;
    let rows = bounded::array(map.get("recorded").ok_or(Error::InvalidReply)?, MAX_REPORTS)?;
    if rows.len() != expected {
        return Err(Error::InvalidReply);
    }
    let mut seen = BTreeSet::new();
    rows.iter()
        .map(|v| {
            let row = bounded::object(v)?;
            let id = bounded::text(row.get("id").ok_or(Error::InvalidReply)?, 12)?;
            crate::astrocollab::validate_id(&id).map_err(|_| Error::InvalidReply)?;
            if !seen.insert(id.clone()) {
                return Err(Error::InvalidReply);
            }
            let verdict = bounded::object(row.get("verdict").ok_or(Error::InvalidReply)?)?;
            let strings = |key| -> Result<Vec<String>, Error> {
                verdict
                    .get(key)
                    .map(|v| {
                        bounded::array(v, 64)?
                            .iter()
                            .map(|v| bounded::text(v, 512))
                            .collect()
                    })
                    .unwrap_or(Ok(vec![]))
            };
            let optional_text = |key, limit| {
                verdict
                    .get(key)
                    .filter(|v| !v.is_null())
                    .map(|v| bounded::text(v, limit))
                    .transpose()
            };
            let accepted = row
                .get("accepted")
                .and_then(Value::as_bool)
                .ok_or(Error::InvalidReply)?;
            let verdict_accepted = verdict
                .get("accepted")
                .and_then(Value::as_bool)
                .ok_or(Error::InvalidReply)?;
            if accepted != verdict_accepted {
                return Err(Error::InvalidReply);
            }
            Ok(Recorded {
                id,
                accepted,
                duplicate: row
                    .get("duplicate")
                    .and_then(Value::as_bool)
                    .ok_or(Error::InvalidReply)?,
                verdict: Verdict {
                    accepted: verdict_accepted,
                    reasons: strings("reasons")?,
                    unverified: strings("unverified")?,
                    summary: optional_text("summary", 1024)?,
                    overridden_by: optional_text("overriddenBy", 512)?,
                    override_reason: optional_text("overrideReason", 1024)?,
                },
            })
        })
        .collect()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LiveStatus {
    pub observed_at_ms: u64,
    pub valid_for_ms: u64,
    pub state: String,
    pub target: Option<String>,
    pub project: Option<String>,
    pub telescope: Option<String>,
    pub ra_hours: Option<f64>,
    pub dec_degrees: Option<f64>,
}

/// Presence is coalesced, never queued for offline replay. Unknown pointing
/// stays null; callers may omit identifying fields for privacy.
pub fn presence(status: &LiveStatus, now_ms: u64) -> Result<Option<Value>, Error> {
    let age = now_ms
        .checked_sub(status.observed_at_ms)
        .ok_or(Error::InvalidEvidence)?;
    if status.valid_for_ms == 0 || age > status.valid_for_ms.min(MAX_PRESENCE_AGE_MS) {
        return Ok(None);
    }
    if status
        .ra_hours
        .is_some_and(|v| !finite(v, 0.0, 24.0) || v == 24.0)
        || status.dec_degrees.is_some_and(|v| !finite(v, -90.0, 90.0))
        || status.ra_hours.is_some() != status.dec_degrees.is_some()
    {
        return Err(Error::InvalidEvidence);
    }
    for text in [
        Some(&status.state),
        status.target.as_ref(),
        status.project.as_ref(),
        status.telescope.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        bounded::text(&json!(text), 120)?;
    }
    if status.state.is_empty() {
        return Err(Error::InvalidEvidence);
    }
    let mut wire = json!({"ra": status.ra_hours, "dec": status.dec_degrees, "state": status.state});
    for (key, value) in [
        ("target", &status.target),
        ("project", &status.project),
        ("telescope", &status.telescope),
    ] {
        if let Some(value) = value {
            wire[key] = json!(value);
        }
    }
    Ok(Some(wire))
}
