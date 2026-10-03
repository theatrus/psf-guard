//! Each activated panel's latest stack preview, placed by its plate solve.

use super::activation::activated;
use super::*;
use crate::astrometry::{
    AstrometryAnalysis, AstrometryAnalysisStatus, AstrometryAttemptOutcome,
    AstrometrySolutionResponse, AstrometrySolveAttempt, AstrometrySolveMode,
    AstrometrySourceFingerprint, WcsResponse,
};

/// A ready H-alpha stack of one target in the rig's cache, in the shape the
/// stack preview job writes, whose reference frame 42 has a persisted solve.
fn write_stack(cache: &std::path::Path, project_row: i64, target_id: i64, solved: bool) {
    let stacks = crate::server::storage::stacks(cache);
    std::fs::create_dir_all(&stacks).unwrap();
    let group = json!({
        "index": 0, "target_id": target_id, "target_name": "IC 1805 r1c1", "filter_name": "Ha",
        "state": "ready", "total_candidates": 3, "eligible_frames": 3, "quality_excluded": 0, "missing_files": 0,
        "processed_frames": 3, "accepted_frames": 3, "rejected_frames": 0,
        "reference_image_id": 42, "total_exposure_seconds": 900.0,
        "preview_url": "/api/db/rig/stack-previews/job-1/0/preview?v=1", "fits_url": null, "error": null, "frames": [],
        "sky_orientation": {"convention": "source_frame", "version": seiza_stacking::SKY_ORIENTATION_VERSION, "source": "test",
            "output_width": 200, "output_height": 100,
            "source_to_output": {"matrix": [[1.0, 0.0], [0.0, 1.0]], "translation_x": 0.0, "translation_y": 0.0}},
    });
    let index = json!({
        "schema_version": 1, "database_id": "rig", "project_id": project_row, "updated_unix_seconds": 1,
        "groups": [{"job_id": "job-1", "artifact_revision": "r1", "accepted_only": false, "created_unix_seconds": 1, "group": group}],
    });
    std::fs::write(
        stacks.join(format!("latest-project-{project_row}.json")),
        index.to_string(),
    )
    .unwrap();
    if !solved {
        return;
    }
    // A north-up, east-left solve one arcsecond per pixel, centered near the panel.
    let analysis = AstrometryAnalysis {
        image_id: 42,
        status: AstrometryAnalysisStatus::Solved,
        mode: Some(AstrometrySolveMode::Hinted),
        hint_source: None,
        expected_source: None,
        solution: Some(AstrometrySolutionResponse {
            center_ra_deg: 38.2,
            center_dec_deg: 62.2,
            pixel_scale_arcsec_per_pixel: 1.0,
            matched_stars: 50,
            rms_arcsec: 0.3,
            image_width: 200,
            image_height: 100,
            wcs: WcsResponse {
                crval: [38.2, 62.2],
                crpix: [99.5, 49.5],
                cd: [[-1.0 / 3600.0, 0.0], [0.0, 1.0 / 3600.0]],
                ctype: ["RA---TAN".into(), "DEC--TAN".into()],
                cunit: ["deg".into(), "deg".into()],
                radesys: "ICRS".into(),
                equinox: 2000.0,
            },
            footprint: vec![],
            objects: vec![],
            catalog_version: None,
            capture_time: None,
        }),
        catalog_hits: vec![],
        catalog_scope: None,
        catalog_radius_deg: None,
        pointing: None,
        source_fingerprint: AstrometrySourceFingerprint {
            canonical_path: "/nowhere/42.fits".into(),
            size_bytes: 1,
            modified_unix_seconds: 1,
            modified_subsec_nanos: 0,
        },
        catalog_signature: None,
        solver_provenance: None,
        solve_attempt: Some(AstrometrySolveAttempt {
            outcome: AstrometryAttemptOutcome::Solved,
            modes_attempted: vec![AstrometrySolveMode::Hinted],
            detected_stars: Some(80),
            duration_ms: 5,
            image_quality_evidence: true,
            cacheable: true,
        }),
        computed_at: 1,
        error: None,
    };
    crate::astrometry::persist_pixel_analysis(cache, &analysis).unwrap();
}

#[tokio::test]
async fn mosaic_places_each_panels_stack_by_its_solve_and_names_the_rest() {
    let a = activated().await;
    let path = format!("/projects/{}/mosaic", a.project);
    // Before activation there are no panels, only the reason.
    let (status, empty) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["data"]["activation_revision"], Value::Null);
    assert_eq!(empty["data"]["panels"], json!([]));
    assert!(empty["data"]["warnings"][0]
        .as_str()
        .unwrap()
        .contains("Activate"));

    let (_, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let (project_row, r1c1): (i64, i64) = a.db.query_row(
        "SELECT p.Id, t.Id FROM target t JOIN project p ON p.Id = t.projectid WHERE t.name = 'IC 1805 r1c1'", [],
        |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
    a.db.execute(
        "UPDATE exposureplan SET acquired = 10, accepted = 8 WHERE targetid = ?1",
        [r1c1],
    )
    .unwrap();

    // No stack yet: both panels say so, with their progress.
    let (status, bare) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{bare}");
    let panels = bare["data"]["panels"].as_array().unwrap();
    assert_eq!(panels.len(), 2);
    assert_eq!(panels[0]["panel_id"], "r1c1");
    assert_eq!(panels[0]["status"], "no_stack");
    assert_eq!(panels[0]["catalog_slug"], "rig");
    assert_eq!(panels[0]["target_id"], r1c1);
    assert_eq!(
        panels[0]["progress"],
        json!({"desired": 72, "acquired": 10, "accepted": 8})
    );
    assert_eq!(panels[1]["panel_id"], "r2c1");
    assert_eq!(panels[1]["progress"]["acquired"], 0);
    assert_eq!(bare["data"]["framing_stale"], false);

    // A stack without a solve exists but cannot be placed.
    let cache =
        a.f.state
            .get_database("rig")
            .unwrap()
            .cache_dir_path
            .clone();
    write_stack(&cache, project_row, r1c1, false);
    let (_, unsolved) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(
        unsolved["data"]["panels"][0]["status"], "unsolved",
        "{unsolved}"
    );
    assert_eq!(unsolved["data"]["panels"][0]["preview"]["wcs"], Value::Null);

    // With its reference frame solved, the stack carries a TAN solution on its grid.
    write_stack(&cache, project_row, r1c1, true);
    let (_, solved) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let panel = &solved["data"]["panels"][0];
    assert_eq!(panel["status"], "ready", "{solved}");
    assert_eq!(panel["preview"]["kind"], "mono");
    assert_eq!(panel["preview"]["filter"], "Ha");
    assert_eq!(panel["preview"]["width"], 200);
    assert_eq!(
        panel["preview"]["url"],
        "/api/db/rig/stack-previews/job-1/0/preview?v=1"
    );
    assert_eq!(panel["preview"]["wcs"]["crval1"], 38.2);
    assert!((panel["preview"]["wcs"]["cd11"].as_f64().unwrap() + 1.0 / 3600.0).abs() < 1e-12);
    assert_eq!(solved["data"]["panels"][1]["status"], "no_stack");

    // A framing saved after the activation is flagged, not hidden.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut draft = store.framing_draft(a.project).unwrap().unwrap();
        let revision = draft.revision;
        draft.position_angle_degrees = 20.0;
        store.save_framing_draft(&draft, revision).unwrap();
    }
    let (_, moved) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(moved["data"]["framing_stale"], true);
    assert_eq!(moved["data"]["panels"][0]["status"], "ready");
    assert_eq!(
        call(
            &a.f.app,
            "GET",
            &format!("/projects/{}/mosaic", Uuid::new_v4()),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    let _ = (a.rig, a.objective);
}
