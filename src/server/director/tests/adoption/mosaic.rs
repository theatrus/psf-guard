//! Each panel's finished stacks, placed by their plate solves.

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
    // Before activation, with no linked project, there are no panels, only the reason.
    let (status, empty) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["data"]["activation_revision"], Value::Null);
    assert_eq!(empty["data"]["panels"], json!([]));
    assert!(empty["data"]["warnings"][0]
        .as_str()
        .unwrap()
        .contains("activate the plan"));

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

/// A plan taken in from Target Scheduler that already matches it is never
/// activated; its linked project's targets are the panels, and every
/// finished stack of each is offered, the most integration first.
#[tokio::test]
async fn a_linked_project_shows_its_stacks_without_an_activation() {
    let a = activated().await;
    let source = Uuid::new_v4();
    a.db.execute(
        "INSERT INTO project (Id, profileId, name, description, state, priority, isMosaic, flatsHandling, guid)
         VALUES (7, 'profile-a', 'M31 by hand', '', 1, 1, 0, 0, ?1)",
        [source.to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO target (Id, name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (5, 'M31 Panel 1', 1, 0.7, 41.3, 2, 0.0, 100, 7, ?1)",
        [Uuid::new_v4().to_string()],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId)
         VALUES ('profile-a', 300, 40, 12, 10, 5, 1)",
        [],
    )
    .unwrap();
    let catalog = crate::catalog_identity::read(&a.db).unwrap().unwrap().id;
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        store
            .link_catalog_project(&psf_guard_director_meta::catalog::ProjectMapping {
                catalog_id: catalog,
                source_project_guid: source,
                source_profile_id: "profile-a".into(),
                project_id: a.project,
                rig_id: a.rig,
            })
            .unwrap();
    }
    let cache =
        a.f.state
            .get_database("rig")
            .unwrap()
            .cache_dir_path
            .clone();
    let stacks = crate::server::storage::stacks(&cache);
    std::fs::create_dir_all(&stacks).unwrap();
    let group = |index: usize, filter: &str, seconds: f64| {
        json!({"job_id": format!("job-{filter}"), "artifact_revision": "r1", "accepted_only": false, "created_unix_seconds": 1, "group": {
            "index": index, "target_id": 5, "target_name": "M31 Panel 1", "filter_name": filter,
            "state": "ready", "total_candidates": 3, "eligible_frames": 3, "quality_excluded": 0, "missing_files": 0,
            "processed_frames": 3, "accepted_frames": 3, "rejected_frames": 0,
            "reference_image_id": 42, "total_exposure_seconds": seconds,
            "preview_url": format!("/api/db/rig/stack-previews/job-{filter}/{index}/preview?v=1"), "fits_url": null, "error": null, "frames": [],
        }})
    };
    std::fs::write(
        stacks.join("latest-project-7.json"),
        json!({"schema_version": 1, "database_id": "rig", "project_id": 7, "updated_unix_seconds": 1,
            "groups": [group(0, "Ha", 900.0), group(1, "OIII", 1800.0)]}).to_string(),
    )
    .unwrap();

    let (status, mosaic) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/mosaic", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{mosaic}");
    let data = &mosaic["data"];
    assert_eq!(data["activation_revision"], Value::Null);
    assert_eq!(data["warnings"], json!([]));
    let panels = data["panels"].as_array().unwrap();
    assert_eq!(panels.len(), 1, "{mosaic}");
    let panel = &panels[0];
    // The lone target is the framing's single panel, so its frames lock that panel.
    assert_eq!(panel["panel_id"], "r1c1");
    assert_eq!(panel["target_name"], "M31 Panel 1");
    assert_eq!(panel["target_id"], 5);
    assert_eq!(
        panel["progress"],
        json!({"desired": 40, "acquired": 12, "accepted": 10})
    );
    let keys: Vec<&str> = panel["stacks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["mono:OIII", "mono:Ha"]);
    assert_eq!(panel["stacks"][1]["label"], "Ha");
    assert_eq!(panel["preview"]["key"], "mono:OIII");
    assert_eq!(panel["status"], "unsolved");
}

/// A frame's FITS header with a TAN solution: `width` by `height` pixels of
/// `scale` arcseconds, centered on `center`, camera up at `angle` east of
/// north, as the frame lies in the file.
fn write_solved_header(
    path: &std::path::Path,
    (width, height): (u32, u32),
    scale: f64,
    center: (f64, f64),
    angle: f64,
) {
    let s = scale / 3600.0;
    let (sin, cos) = angle.to_radians().sin_cos();
    let cards = [
        "SIMPLE  =                    T".to_string(),
        "BITPIX  =                   16".into(),
        "NAXIS   =                    2".into(),
        format!("NAXIS1  = {width:>20}"),
        format!("NAXIS2  = {height:>20}"),
        "CTYPE1  = 'RA---TAN'".into(),
        "CTYPE2  = 'DEC--TAN'".into(),
        format!("CRVAL1  = {:>20.10}", center.0),
        format!("CRVAL2  = {:>20.10}", center.1),
        format!("CRPIX1  = {:>20.4}", (f64::from(width) + 1.0) / 2.0),
        format!("CRPIX2  = {:>20.4}", (f64::from(height) + 1.0) / 2.0),
        format!("CD1_1   = {:>20.12E}", -s * cos),
        format!("CD1_2   = {:>20.12E}", -s * sin),
        format!("CD2_1   = {:>20.12E}", s * sin),
        format!("CD2_2   = {:>20.12E}", -s * cos),
        "END".into(),
    ];
    let mut header = Vec::new();
    for card in &cards {
        let mut bytes = card.as_bytes().to_vec();
        bytes.resize(80, b' ');
        header.extend(bytes);
    }
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    std::fs::write(path, header).unwrap();
}

/// A fresh pixel solve of the frame at `path`, the way the quality scan
/// persists it.
fn persist_solve(
    cache: &std::path::Path,
    image_id: i32,
    path: &std::path::Path,
    (width, height): (u32, u32),
    scale: f64,
    center: (f64, f64),
    angle: f64,
) {
    let s = scale / 3600.0;
    let (sin, cos) = angle.to_radians().sin_cos();
    let analysis = AstrometryAnalysis {
        image_id,
        status: AstrometryAnalysisStatus::Solved,
        mode: Some(AstrometrySolveMode::Hinted),
        hint_source: None,
        expected_source: None,
        solution: Some(AstrometrySolutionResponse {
            center_ra_deg: center.0,
            center_dec_deg: center.1,
            pixel_scale_arcsec_per_pixel: scale,
            matched_stars: 50,
            rms_arcsec: 0.3,
            image_width: width,
            image_height: height,
            wcs: WcsResponse {
                crval: [center.0, center.1],
                crpix: [
                    (f64::from(width) - 1.0) / 2.0,
                    (f64::from(height) - 1.0) / 2.0,
                ],
                cd: [[-s * cos, -s * sin], [s * sin, -s * cos]],
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
        source_fingerprint: crate::astrometry::source_fingerprint(path).unwrap(),
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

fn measured_of<'a>(mosaic: &'a Value, panel: &str) -> &'a Value {
    mosaic["data"]["measured"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["panel_id"] == panel && m["framing_rig_id"].is_null())
        .unwrap_or_else(|| panic!("{panel} measured: {mosaic}"))
}

/// A shot panel is drawn where its frames say: from fresh solves, else
/// their headers, else the target as planned. A locked panel keeps that
/// geometry, a save cannot make it up, and activation leaves its target
/// where it was shot.
#[tokio::test]
async fn a_shot_panel_is_measured_from_its_frames_and_its_lock_holds_it() {
    let a = activated().await;
    let dir = a.f._dir.path().to_path_buf();
    // Two frames of the upper panel on disk, shot with another camera than
    // the plan's and turned to 25°; the lower panel's frames are not.
    let (r1_ra, r1_dec) = (38.25, 61.95);
    for n in 1..=2 {
        write_solved_header(
            &dir.join(format!("frame-{n}.fits")),
            (4000, 3000),
            2.0,
            (r1_ra, r1_dec),
            25.0,
        );
    }
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
    let target = |name: &str| -> (i64, i64, f64, f64, f64) {
        a.db.query_row(
            "SELECT Id, projectid, ra, dec, rotation FROM target WHERE name = ?1",
            [name],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )
        .unwrap()
    };
    let (r1c1, project_row, ..) = target("IC 1805 r1c1");
    let (r2c1, _, r2_ra, r2_dec, r2_rotation) = target("IC 1805 r2c1");
    a.db.execute_batch(&format!(
        r#"UPDATE exposureplan SET acquired = 3;
         INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata) VALUES
           (101, {project_row}, {r1c1}, 1, 'Ha', 1, '{{"FileName":"C:\\shots\\frame-1.fits"}}'),
           (102, {project_row}, {r1c1}, 2, 'Ha', 0, '{{"FileName":"frame-2.fits"}}'),
           (103, {project_row}, {r1c1}, 3, 'Ha', 2, '{{"FileName":"frame-9.fits"}}'),
           (201, {project_row}, {r2c1}, 1, 'Ha', 1, '{{"FileName":"gone-1.fits"}}'),
           (202, {project_row}, {r2c1}, 2, 'Ha', 1, '{{"FileName":"gone-2.fits"}}');"#
    ))
    .unwrap();
    let path = format!("/projects/{}/mosaic", a.project);

    // No solves yet: the headers say where the upper panel was shot, and
    // the rejected frame is not counted. The lower panel has only its plan.
    let (status, read) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    let upper = measured_of(&read, "r1c1")["geometry"].clone();
    assert_eq!(upper["source"], "header", "{upper}");
    assert_eq!(upper["size_source"], "header");
    assert_eq!(upper["frames"], 2);
    assert!((upper["center"]["ra_degrees"].as_f64().unwrap() - r1_ra).abs() < 1e-6);
    assert!((upper["center"]["dec_degrees"].as_f64().unwrap() - r1_dec).abs() < 1e-6);
    assert!((upper["position_angle_degrees"].as_f64().unwrap() - 25.0).abs() < 1e-6);
    assert!(
        (upper["size"]["width_degrees"].as_f64().unwrap() - 4000.0 * 2.0 / 3600.0).abs() < 1e-9
    );
    assert!(
        (upper["size"]["height_degrees"].as_f64().unwrap() - 3000.0 * 2.0 / 3600.0).abs() < 1e-9
    );
    let lower = &measured_of(&read, "r2c1")["geometry"];
    assert_eq!(lower["source"], "plan", "{lower}");
    assert_eq!(lower["size_source"], "plan");
    assert_eq!(lower["frames"], 2);
    assert!((lower["center"]["ra_degrees"].as_f64().unwrap() - r2_ra * 15.0).abs() < 1e-9);
    assert!((lower["center"]["dec_degrees"].as_f64().unwrap() - r2_dec).abs() < 1e-9);
    assert!((lower["position_angle_degrees"].as_f64().unwrap() - r2_rotation).abs() < 1e-9);
    // Without a field of its own, the panel the plan draws.
    assert_eq!(
        lower["size"],
        json!({"width_degrees": 2.0, "height_degrees": 1.5})
    );

    // Fresh solves outrank the headers: the median of them.
    let cache =
        a.f.state
            .get_database("rig")
            .unwrap()
            .cache_dir_path
            .clone();
    for (id, n, angle) in [(101, 1, 26.0), (102, 2, 26.4)] {
        persist_solve(
            &cache,
            id,
            &dir.join(format!("frame-{n}.fits")),
            (200, 150),
            36.0,
            (r1_ra + 0.01, r1_dec),
            angle,
        );
    }
    let (_, read) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let solved = measured_of(&read, "r1c1")["geometry"].clone();
    assert_eq!(solved["source"], "solved", "{solved}");
    assert_eq!(solved["size_source"], "solved");
    assert_eq!(solved["frames"], 2);
    assert!((solved["position_angle_degrees"].as_f64().unwrap() - 26.2).abs() < 1e-9);
    assert!((solved["size"]["width_degrees"].as_f64().unwrap() - 2.0).abs() < 1e-9);
    assert!((solved["size"]["height_degrees"].as_f64().unwrap() - 1.5).abs() < 1e-9);

    // The editor locks the panel; whatever shot geometry the body carries,
    // the server writes its own reading. A client that sends none, or one
    // stale or made up, gets the same.
    let framing = format!("/projects/{}/framing", a.project);
    let (_, got) = call(&a.f.app, "GET", &framing, Value::Null, None).await;
    let mut draft = got["data"]["draft"].clone();
    draft["layout"]["panels"][0]["locked"] = json!(true);
    let mut made_up = draft.clone();
    let mut wrong = solved.clone();
    wrong["frames"] = json!(9);
    wrong["position_angle_degrees"] = json!(120.0);
    made_up["layout"]["panels"][0]["measured"] = wrong;
    let (status, saved) = call(&a.f.app, "PUT", &framing, made_up, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let saved = saved["data"]["draft"].clone();
    assert_eq!(saved["layout"]["panels"][0]["measured"], solved);
    // Without any, the same reading; an unlocked panel is planned and has none.
    let mut bare = saved.clone();
    bare["layout"]["panels"][0]
        .as_object_mut()
        .unwrap()
        .remove("measured");
    bare["layout"]["panels"][1]["measured"] = solved.clone();
    let (status, again) = call(&a.f.app, "PUT", &framing, bare, None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let saved = again["data"]["draft"].clone();
    assert_eq!(saved["layout"]["panels"][0]["measured"], solved);
    assert!(
        saved["layout"]["panels"][1].get("measured").is_none(),
        "{saved}"
    );
    // A new solve moves the reading on; the next save carries it, though
    // the body still has the old one.
    persist_solve(
        &cache,
        102,
        &dir.join("frame-2.fits"),
        (200, 150),
        36.0,
        (r1_ra + 0.01, r1_dec),
        27.0,
    );
    let (status, newer) = call(&a.f.app, "PUT", &framing, saved.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{newer}");
    let saved = newer["data"]["draft"].clone();
    assert!(
        (saved["layout"]["panels"][0]["measured"]["position_angle_degrees"]
            .as_f64()
            .unwrap()
            - 26.5)
            .abs()
            < 1e-9,
        "{saved}"
    );

    // Turning the locked panel is a move, refused like any other.
    let mut turned = saved.clone();
    turned["layout"]["panels"][0]["angle_offset_degrees"] = json!(5.0);
    let (status, refused) = call(&a.f.app, "PUT", &framing, turned, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["error"]
            .as_str()
            .unwrap()
            .contains("Locked panel r1c1 would move. A locked panel keeps its place, its turn"),
        "{refused}"
    );

    // Activation leaves the locked panel's target where it was shot.
    let (status, again) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{again}");
    let changes = again["data"]["rigs"][0]["changes"].as_array().unwrap();
    let upper = changes
        .iter()
        .find(|c| c["kind"] == "target" && c["name"] == "IC 1805 r1c1")
        .unwrap();
    assert_eq!(upper["action"], "unchanged", "{again}");
    assert!(
        upper["detail"]
            .as_str()
            .unwrap()
            .contains("kept where its frames were shot"),
        "{upper}"
    );
    let _ = (a.rig, a.objective);
}
