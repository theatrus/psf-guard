//! Synthetic protocol vectors for the independent AstroCollab schema checker.
//! This does not read real images, send HTTP or authorize acquisition.
#[path = "../tests/support/mod.rs"]
mod support;
use psf_guard_director_interop::collaboration::{finalize_contribution, presence, LiveStatus};
use serde_json::json;

fn main() {
    let plan = support::import();
    let report = finalize_contribution(&plan, &[support::frame(&plan, 1)]).unwrap();
    let status = LiveStatus {
        observed_at_ms: 1000,
        valid_for_ms: 10_000,
        state: "imaging".into(),
        target: None,
        project: None,
        telescope: None,
        ra_hours: Some(2.0),
        dec_degrees: Some(30.0),
    };
    println!(
        "{}",
        json!({
            "report": {"contributions": [report.report()]},
            "presence": presence(&status, 2000).unwrap().unwrap(),
        })
    );
}
