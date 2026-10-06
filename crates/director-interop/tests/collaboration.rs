mod support;
use psf_guard_director_interop::{collaboration::*, Error};
use serde_json::json;
use support::*;

#[test]
fn import_round_trip_preserves_identity_and_strips_unknown_secrets() {
    let mut v = wire();
    v["token"] = json!("do-not-persist");
    v["tasks"][0]["token"] = json!("do-not-persist");
    v.as_object_mut().unwrap().remove("task");
    let p = prepared(&v);
    assert!(!String::from_utf8_lossy(p.snapshot()).contains("do-not-persist"));
    let read = prepare_import(p.snapshot(), p.source(), p.night(), &p.share().task_id).unwrap();
    assert_eq!(read.digest(), p.digest());
    assert_eq!(read.share().demands, p.share().demands);
    assert_eq!(read.share().geometry_digest, p.share().geometry_digest);
    assert_eq!(read.project_id(), p.project_id());
    assert_eq!(read.import_id(), p.import_id());
}

#[test]
fn project_identity_rolls_up_agents_but_import_identity_keeps_the_rig_and_night() {
    let a = import();
    let source_b = psf_guard_director_interop::astrocollab::Source::new(
        source().base_url(),
        "000000000004",
        false,
    )
    .unwrap();
    let mut v = wire();
    v.as_object_mut().unwrap().remove("task");
    v["tasks"][0]["agent"] = json!(source_b.agent_id());
    let b = prepare_import(
        &serde_json::to_vec(&v).unwrap(),
        &source_b,
        NIGHT,
        &a.share().task_id,
    )
    .unwrap();
    assert_eq!(a.project_id(), b.project_id());
    assert_ne!(a.import_id(), b.import_id());
    v["tasks"][0]["assignedNight"] = json!("2026-10-06");
    let c = prepare_import(
        &serde_json::to_vec(&v).unwrap(),
        &source_b,
        "2026-10-06",
        &a.share().task_id,
    )
    .unwrap();
    assert_eq!(c.project_id(), b.project_id());
    assert_ne!(c.import_id(), b.import_id());
}

#[test]
fn held_work_cannot_be_prepared_for_import() {
    for edit in ["offered", "complete", "superseded"] {
        let mut v = wire();
        v.as_object_mut().unwrap().remove("task");
        v["tasks"][0]["state"] = json!(edit);
        assert!(matches!(
            prepare_import(&serde_json::to_vec(&v).unwrap(), &source(), NIGHT, TASK),
            Err(Error::NeedsReview)
        ));
    }
}

#[test]
fn contribution_uses_original_night_actual_saved_exposures_and_pixel_footprint() {
    let p = import();
    let a = frame(&p, 1);
    let b = frame(&p, 2);
    let r = finalize_contribution(&p, &[b.clone(), a.clone()]).unwrap();
    assert_eq!(r.captures(), &[a.capture_id, b.capture_id]);
    assert_eq!(r.images(), &[a.image_guid, b.image_guid]);
    let v = serde_json::to_value(r.report()).unwrap();
    assert_eq!(v["night"], NIGHT);
    assert_eq!(v["panel"], "0");
    assert_eq!(v["filterName"], "O");
    assert_eq!(v["seconds"], 600.0);
    assert_eq!(v["exposure"], 300.0);
    assert_eq!(v["footprint"]["ra"], 10.5);
    assert_ne!(
        v["footprint"]["ra"],
        json!(p.share().region.icrs_ra_mas as f64 / 3_600_000.0)
    );
}

#[test]
fn failed_uncertain_rejected_unfinalized_or_stale_solve_frames_cannot_earn_credit() {
    let p = import();
    let f = frame(&p, 1);
    let edits: &[fn(&mut FrameEvidence)] = &[
        |f| f.saved = false,
        |f| f.accepted = false,
        |f| f.finalized = false,
        |f| f.source_digest = "wrong-revision".into(),
        |f| f.solve_fingerprint = "old-file".into(),
        |f| f.image_fingerprint.clear(),
        |f| f.image_fingerprint = "invalid\nfingerprint".into(),
        |f| f.exposure_ms = 60_000,
        |f| f.panel_index = 255,
        |f| f.filter = "H-alpha".into(),
        |f| f.scale_arcsec = Some(f64::NAN),
        |f| f.scale_arcsec = Some(0.0),
        |f| f.focal_length_mm = Some(0.0),
        |f| f.bandpass_nm = Some(0.0),
        |f| f.solved_footprint.ra = 360.0,
        |f| f.image_guid = uuid::Uuid::nil(),
    ];
    for edit in edits {
        let mut bad = f.clone();
        edit(&mut bad);
        assert!(finalize_contribution(&p, &[bad]).is_err());
    }
    assert!(finalize_contribution(&p, &[f.clone(), f]).is_err());
}

#[test]
fn incomplete_metrics_stay_unknown_and_heterogeneous_cohorts_are_refused() {
    let p = import();
    let a = frame(&p, 1);
    let mut b = frame(&p, 2);
    b.hfr_arcsec = None;
    let report = finalize_contribution(&p, &[a.clone(), b.clone()]).unwrap();
    assert!(serde_json::to_value(report.report()).unwrap()["hfr"].is_null());
    b.colour = true;
    assert!(finalize_contribution(&p, &[a.clone(), b.clone()]).is_err());
    b.colour = false;
    b.bandpass_nm = Some(7.0);
    assert!(finalize_contribution(&p, &[a.clone(), b.clone()]).is_err());
    b.bandpass_nm = a.bandpass_nm;
    b.solved_footprint.ra += 0.001;
    assert!(finalize_contribution(&p, &[a, b]).is_err());
}

#[test]
fn calibration_is_verified_not_inferred_from_available_masters() {
    let mut v = wire();
    v["requirementsByProject"]["000000000002"]["requireCalibrated"] = json!(true);
    let p = prepared(&v);
    assert!(matches!(
        finalize_contribution(&p, &[frame(&p, 1)]),
        Err(Error::NeedsReview)
    ));
    let mut good = frame(&p, 1);
    good.calibrated = true;
    assert!(finalize_contribution(&p, &[good]).is_ok());
}

#[test]
fn entire_positional_reply_is_validated_before_acknowledgement() {
    let mut good = json!({"recorded":[reply(1,true),reply(2,false)],"future":true});
    good["recorded"][0]["verdict"]["overriddenBy"] = json!("Coordinator");
    good["recorded"][0]["verdict"]["overrideReason"] = json!("Reviewed the stack");
    let recorded = decode_recorded(&serde_json::to_vec(&good).unwrap(), 2).unwrap();
    assert_eq!(
        recorded[0].verdict.overridden_by.as_deref(),
        Some("Coordinator")
    );
    assert_eq!(
        recorded[0].verdict.override_reason.as_deref(),
        Some("Reviewed the stack")
    );
    assert_eq!(
        decode_recorded(&serde_json::to_vec(&good).unwrap(), 2)
            .unwrap()
            .len(),
        2
    );
    assert!(decode_recorded(&serde_json::to_vec(&good).unwrap(), 1).is_err());
    for value in [
        json!({"recorded":[reply(1,true)]}),
        json!({"recorded":[reply(1,true),reply(1,true)]}),
        json!({"recorded":[reply(1,true),{"id":"000000000002"}]}),
    ] {
        assert!(decode_recorded(&serde_json::to_vec(&value).unwrap(), 2).is_err());
    }
    let mut bad = good.clone();
    bad["recorded"][0]["verdict"]["accepted"] = json!(false);
    assert!(decode_recorded(&serde_json::to_vec(&bad).unwrap(), 2).is_err());
}

#[test]
fn live_presence_is_fresh_only_and_uses_ra_hours_not_planned_degrees() {
    let mut status = LiveStatus {
        observed_at_ms: 1000,
        valid_for_ms: 10_000,
        state: "imaging".into(),
        target: None,
        project: None,
        telescope: None,
        ra_hours: Some(2.0),
        dec_degrees: Some(30.0),
    };
    let p = presence(&status, 2000).unwrap().unwrap();
    assert_eq!(p["ra"], 2.0);
    assert!(!p.as_object().unwrap().contains_key("target"));
    assert!(!p.as_object().unwrap().contains_key("project"));
    assert!(!p.as_object().unwrap().contains_key("telescope"));
    assert!(presence(&status, 11_001).unwrap().is_none());
    status.valid_for_ms = u64::MAX;
    assert!(presence(&status, 61_001).unwrap().is_none());
    assert!(presence(&status, 999).is_err());
    status.ra_hours = Some(24.0);
    assert!(presence(&status, 2000).is_err());
}
