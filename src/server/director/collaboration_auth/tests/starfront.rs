use super::*;
use psf_guard_director_interop::collaboration::{
    finalize_contribution, prepare_import, FrameEvidence, MeasuredFootprint,
};

/// Synthetic evidence is sent only to a throwaway loopback server, never to
/// a shared service. The owner credential is read from its local token file.
#[tokio::test]
#[ignore = "Requires STARFRONT_TEST_URL and STARFRONT_TEST_OWNER_FILE for a throwaway loopback server"]
async fn starfront_server_hello_join_import_report_and_replay() {
    let endpoint = std::env::var("STARFRONT_TEST_URL").expect("Set throwaway server URL");
    let url = reqwest::Url::parse(&endpoint).unwrap();
    assert_eq!(url.scheme(), "http");
    assert_eq!(url.host_str(), Some("127.0.0.1"));
    let owner = std::fs::read_to_string(
        std::env::var("STARFRONT_TEST_OWNER_FILE").expect("Set local owner token file"),
    )
    .unwrap();
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()
        .unwrap();
    for colour in [false, true] {
        let enrolled = http
            .post(url.join("api/v1/agents").unwrap())
            .bearer_auth(owner.trim())
            .json(&json!({"name":"PSF Guard compatibility TEST ONLY"}))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .bytes()
            .await
            .unwrap();
        let project: Value = http
        .post(url.join("api/v1/projects").unwrap())
        .bearer_auth(owner.trim())
        .json(&json!({
            "name":"PSF Guard compatibility TEST ONLY", "kind":"single",
            "region":{"ra":10.6847,"dec":41.269,"width":3.0,"height":2.0,"rotation":0},
            "requirements":{"filters":if colour {json!({"OSC-RGB":null})} else {json!({"H":7,"O":7})},"minExposure":120,"maxExposure":600,
                "minFramesPerVisit":1,"acceptOscRgb":colour,"acceptOscNarrowband":true,"maxBortle":5},
            "goals":if colour {json!({"OSC-RGB":10})} else {json!({"H":10,"O":10})}
        }))
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
        let project = project["project"]["id"].as_str().unwrap().to_owned();
        let (_dir, state, app, rig) = fixture().await;
        commissioned(&state, rig).await;
        let id = add(&app, rig, &endpoint, &[]).await;
        let service = state.director.clone().unwrap();
        let binding = service
            .clone()
            .query(move |s| s.collaboration_connection(id))
            .await
            .unwrap()
            .unwrap();
        let binding = persist_enrollment(
            service.clone(),
            registry(&state).unwrap(),
            binding,
            &enrolled,
        )
        .await
        .unwrap();
        let source = binding.source().unwrap();
        let route = format!("/collaboration/{id}/work");
        assert_eq!(
        call(&app, "POST", &route, if colour {
            json!({"operation":"configure","settings":{"binning":1,"colour":true,"hours_per_night":6,
                "share_status":false,"filters":{"Clear":{"exposure_seconds":120,"bandpass_nm":null}}}})
        } else {setup_filters()}, &[]).await.0,
        StatusCode::OK
    );
        let (status, listed) = call(&app, "POST", &route, json!({"operation":"browse"}), &[]).await;
        assert_eq!(status, StatusCode::OK, "{listed}");
        let listed = listed["data"]["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["project_id"] == project)
            .unwrap();
        assert_eq!(listed["requirements"]["max_bortle"], 5);
        assert_eq!(listed["requirements"]["accept_osc_rgb"], colour);
        let (status, work) = call(
            &app,
            "POST",
            &route,
            json!({"operation":"join","project":project,"night":observing_night()}),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{work}");
        let task = work["data"]["shares"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["project_id"] == project)
            .unwrap()["task_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let (status, preview) = call(
            &app,
            "POST",
            &route,
            json!({"operation":"preview","task":task,"night":observing_night()}),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{preview}");
        let (status, applied) = call(
            &app,
            "POST",
            &route,
            json!({"operation":"apply","task":task,"night":observing_night(),
            "review_digest":preview["data"]["preview"]["review_digest"]}),
            &[],
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{applied}");
        assert_eq!(applied["data"]["preview"]["acquisition_enabled"], false);
        let remote = Remote::new(&binding).unwrap();
        let token = credentials::read(&registry(&state).unwrap(), &binding)
            .unwrap()
            .unwrap();
        let wire = remote
            .send_query(
                "agent/task",
                Some(&token),
                None,
                &[
                    ("night", "2026-10-05".into()),
                    ("moon", "0.12".into()),
                    ("moonUp", "0.3".into()),
                ],
            )
            .await
            .unwrap();
        let plan = prepare_import(&wire, &source, "2026-10-05", &task).unwrap();
        let d = &plan.share().demands[0];
        let cell = &plan.share().cells[d.panel_index as usize].region;
        let frame = FrameEvidence {
            capture_id: Uuid::new_v4(),
            image_guid: Uuid::new_v4(),
            source_digest: plan.digest().into(),
            panel_index: d.panel_index,
            filter: d.filter.clone(),
            exposure_ms: d.exposure_ms,
            saved: true,
            accepted: true,
            finalized: true,
            image_fingerprint: "synthetic-local-test".into(),
            solve_fingerprint: "synthetic-local-test".into(),
            solved_footprint: MeasuredFootprint {
                ra: f64::from(cell.icrs_ra_mas) / 3_600_000.0,
                dec: f64::from(cell.icrs_dec_mas) / 3_600_000.0,
                width: f64::from(cell.width_mas) / 3_600_000.0,
                height: f64::from(cell.height_mas) / 3_600_000.0,
                rotation: f64::from(cell.position_angle_mas) / 3_600_000.0,
            },
            scale_arcsec: Some(1.46),
            focal_length_mm: Some(530.0),
            hfr_arcsec: Some(2.4),
            guide_rms_arcsec: Some(0.6),
            moon_illumination: Some(0.12),
            moon_separation_degrees: Some(90.0),
            calibrated: false,
            bandpass_nm: if colour { None } else { Some(3.0) },
            colour,
        };
        let report = finalize_contribution(&plan, &[frame]).unwrap();
        let payload = serde_json::to_value(report.report()).unwrap();
        if colour {
            assert_eq!(payload["filterName"], "L");
            assert_eq!(payload["channels"], json!(["OSC-RGB"]));
            assert_eq!(payload["exposure"], 120.0);
        }
        let queued_at = workflows::now_ms().unwrap();
        service
            .clone()
            .run(move |s| s.queue_collaboration_report(&report, queued_at))
            .await
            .unwrap();
        let (status, delivered) =
            call(&app, "POST", &route, json!({"operation":"checkin"}), &[]).await;
        assert_eq!(status, StatusCode::OK, "{delivered}");
        assert_eq!(delivered["data"]["delivered"], 1);
        assert_eq!(delivered["data"]["accepted"], 1);
        let (status, empty) = call(&app, "POST", &route, json!({"operation":"checkin"}), &[]).await;
        assert_eq!(status, StatusCode::OK, "{empty}");
        assert_eq!(empty["data"]["delivered"], 0);
        let replay = remote
            .post(
                "agent/report",
                Some(&token),
                json!({"contributions":[payload]}),
            )
            .await
            .unwrap();
        let replay: Value = serde_json::from_slice(&replay).unwrap();
        assert_eq!(replay["recorded"][0]["duplicate"], true);
        assert_eq!(replay["recorded"][0]["accepted"], true);
        eprintln!(
            "Starfront host check passed: {} import, durable report, duplicate replay",
            if colour { "OSC-RGB" } else { "mono" }
        );
    }
}
