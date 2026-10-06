#![allow(dead_code)]
use psf_guard_director_interop::{astrocollab::Source, collaboration::*};
use serde_json::{json, Value};
use uuid::Uuid;

pub const NIGHT: &str = "2026-10-05";
pub const TASK: &str = "000000000004";
pub fn source() -> Source {
    Source::new("https://collab.example/community", "000000000001", false).unwrap()
}
pub fn wire() -> Value {
    serde_json::from_slice(include_bytes!("../fixtures/starfront-tonight.json")).unwrap()
}
pub fn prepared(value: &Value) -> PreparedImport {
    let task = value["tasks"][0]["id"].as_str().unwrap();
    prepare_import(&serde_json::to_vec(value).unwrap(), &source(), NIGHT, task).unwrap()
}
pub fn import() -> PreparedImport {
    prepared(&wire())
}
pub fn frame(import: &PreparedImport, n: u128) -> FrameEvidence {
    FrameEvidence {
        capture_id: Uuid::from_u128(n),
        image_guid: Uuid::from_u128(100_000 - n),
        source_digest: import.digest().into(),
        panel_index: 0,
        filter: "OIII".into(),
        exposure_ms: 300_000,
        saved: true,
        accepted: true,
        finalized: true,
        image_fingerprint: format!("image-{n}-v1"),
        solve_fingerprint: format!("image-{n}-v1"),
        solved_footprint: MeasuredFootprint {
            ra: 10.5,
            dec: 41.0,
            width: 1.25,
            height: 0.8,
            rotation: 0.0,
        },
        scale_arcsec: Some(1.8),
        focal_length_mm: Some(400.0),
        hfr_arcsec: Some(2.5),
        guide_rms_arcsec: Some(0.7),
        moon_illumination: Some(0.05),
        moon_separation_degrees: Some(40.0),
        calibrated: false,
        bandpass_nm: Some(3.0),
        colour: false,
    }
}
pub fn reply(id: u64, accepted: bool) -> Value {
    json!({"id": format!("{id:012x}"), "accepted": accepted, "duplicate":false,
        "verdict": {"accepted":accepted,"reasons":[],"unverified":[],"summary":"test"}})
}
