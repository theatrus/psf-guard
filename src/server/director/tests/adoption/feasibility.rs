use super::activation::activated;
use super::*;

#[tokio::test]
async fn feasibility_times_each_rig_with_a_site_and_names_the_rest() {
    let a = activated().await;
    let path = format!("/projects/{}/feasibility", a.project);
    // 2026-09-25 00:00 UTC keeps the numbers stable.
    let body = json!({"nights": 3, "start_ms": 1_790_294_400_000u64});
    let (status, first) = call(&a.f.app, "POST", &path, body.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["data"]["rigs"], json!([]));
    assert!(
        first["data"]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| {
                let w = w.as_str().unwrap();
                w.contains("no rig profile") || w.contains("no site")
            }),
        "{first}"
    );

    // Give the rig a Los Angeles site and a walled southern horizon.
    let (_, view) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/profile",
        Value::Null,
        None,
    )
    .await;
    let edit = json!({
        "expected_revision": view["data"]["profile"]["revision"],
        "optics": null,
        "site": {"value": {"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0}, "source": {"kind":"manual"}},
        "horizon": {"value": {"mode":"custom","points":[{"azimuth_degrees":0.0,"altitude_degrees":10.0},{"azimuth_degrees":180.0,"altitude_degrees":45.0},{"azimuth_degrees":360.0,"altitude_degrees":10.0}]}, "source": {"kind":"manual"}},
        "sky_quality": null,
        "limits": {"value": {"minimum_altitude_degrees": 25.0, "maximum_altitude_degrees": 90.0, "meridian_exclusion": {"before_ms": 0, "after_ms": 0}}, "source": {"kind":"manual"}},
    });
    let (status, saved) = call(&a.f.app, "PUT", "/catalogs/rig/rig/profile", edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");

    let (status, second) = call(&a.f.app, "POST", &path, body, None).await;
    assert_eq!(status, StatusCode::OK, "{second}");
    let data = &second["data"];
    assert_eq!(data["target_name"], "IC 1805");
    assert_eq!(data["center"]["dec_degrees"], 61.45);
    let rigs = data["rigs"].as_array().unwrap();
    assert_eq!(rigs.len(), 1, "{data}");
    let rig = &rigs[0];
    assert_eq!(rig["rig"]["id"], a.rig.to_string());
    // Target Scheduler leaves a project's custom horizon off until a plan
    // turns it on, and says so.
    assert_eq!(rig["custom_horizon"], false);
    assert!(
        data["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("Custom horizon is off")),
        "{data}"
    );
    assert_eq!(rig["in_plan"], true);
    let nights = rig["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 3);
    // 00:00 UTC is 17:00 the day before in Los Angeles: tonight is the 24th.
    assert_eq!(nights[0]["date"], "2026-09-24");
    assert!(nights[0]["dark_hours"].as_f64().unwrap() > 9.0);
    let up = nights[0]["targets"][0]["hours_up"].as_f64().unwrap();
    assert!(up > 5.0 && up < 10.5, "{up}");
    assert!(nights[0]["moon_illumination"].as_f64().unwrap() > 0.9);
    // Six hours of H-alpha at 300 s over two panels is twelve hours of work.
    assert!(
        (rig["hours_needed"].as_f64().unwrap() - 12.0).abs() < 1e-6,
        "{}",
        rig["hours_needed"]
    );
    let nights_to_complete = rig["nights_to_complete"].as_u64().unwrap();
    assert!(
        (2..=3).contains(&nights_to_complete),
        "{nights_to_complete}"
    );
    let samples = rig["curve"]["samples"].as_array().unwrap();
    assert_eq!(samples.len(), 288);
    assert!(samples
        .iter()
        .all(|s| s["targets"][0]["horizon_altitude_degrees"].is_null()));
    assert!(samples
        .iter()
        .any(|s| s["sun_altitude_degrees"].as_f64().unwrap() < -18.0));

    // A center sent by the browser overrides the saved framing.
    let (status, south) = call(&a.f.app, "POST", &path, json!({"nights": 1, "start_ms": 1_790_294_400_000u64, "center": {"ra_degrees": 38.2, "dec_degrees": -75.0}}), None).await;
    assert_eq!(status, StatusCode::OK, "{south}");
    assert_eq!(
        south["data"]["rigs"][0]["nights"][0]["targets"][0]["hours_up"],
        0.0
    );
    assert_eq!(south["data"]["rigs"][0]["nights_to_complete"], Value::Null);
    assert_eq!(
        call(&a.f.app, "POST", &path, json!({"nights": 40}), None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    // A start the core cannot time is refused, never overflowed.
    for start_ms in [0, 4_070_908_800_001u64, u64::MAX] {
        let (status, refused) =
            call(&a.f.app, "POST", &path, json!({"start_ms": start_ms}), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert!(
            refused["error"].as_str().unwrap().contains("start_ms"),
            "{refused}"
        );
    }
    let bare = {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        store.create_project(Uuid::new_v4(), "Bare").unwrap().id
    };
    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &format!("/projects/{bare}/feasibility"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let _ = a.objective;
}

/// Give the fixture's rig a Los Angeles site, `horizon` and a `minimum`
/// altitude in its profile.
async fn place_in_los_angeles(a: &super::activation::Activated, horizon: Value, minimum: f64) {
    let (_, view) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/profile",
        Value::Null,
        None,
    )
    .await;
    let edit = json!({
        "expected_revision": view["data"]["profile"]["revision"],
        "optics": null,
        "site": {"value": {"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0}, "source": {"kind":"manual"}},
        "horizon": horizon,
        "sky_quality": null,
        "limits": {"value": {"minimum_altitude_degrees": minimum, "maximum_altitude_degrees": 90.0, "meridian_exclusion": {"before_ms": 0, "after_ms": 0}}, "source": {"kind":"manual"}},
    });
    let (status, saved) = call(&a.f.app, "PUT", "/catalogs/rig/rig/profile", edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
}

/// The fixture's rig on its one night from 2026-09-24 17:00 local.
async fn tonight(a: &super::activation::Activated) -> Value {
    let path = format!("/projects/{}/feasibility", a.project);
    let body = json!({"nights": 1, "start_ms": 1_790_294_400_000u64});
    let (status, view) = call(&a.f.app, "POST", &path, body, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    assert_eq!(view["data"]["rigs"].as_array().unwrap().len(), 1, "{view}");
    view["data"]["rigs"][0].clone()
}

fn hours_up(rig: &Value) -> f64 {
    rig["nights"][0]["targets"][0]["hours_up"].as_f64().unwrap()
}

/// The limits activation writes into the rig's Target Scheduler project are
/// the limits the nights are timed with: the plan's minimum over the
/// profile's, its custom horizon and offset, and its meridian window.
#[tokio::test]
async fn feasibility_times_the_nights_with_the_plans_scheduling_limits() {
    use psf_guard_director_core::priority::Scope;
    use psf_guard_director_meta::preferences::Settings;
    let a = activated().await;
    // A wall to the south: 10 degrees at north, 45 at south.
    let horizon = json!({"value": {"mode":"custom","points":[{"azimuth_degrees":0.0,"altitude_degrees":10.0},{"azimuth_degrees":180.0,"altitude_degrees":45.0},{"azimuth_degrees":360.0,"altitude_degrees":10.0}]}, "source": {"kind":"manual"}});
    place_in_los_angeles(&a, horizon, 20.0).await;
    let open = tonight(&a).await;
    assert_eq!(open["limits"]["minimum_altitude_degrees"], 20.0);
    assert_eq!(open["custom_horizon"], false);
    let set = |change: &dyn Fn(&mut Settings)| {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        // Each call replaces the plan's limits, at the stored revision.
        let revision = store
            .observing_settings(Scope::Project, a.project)
            .unwrap()
            .revision;
        let mut project = Settings::empty(Scope::Project, a.project);
        project.revision = revision;
        change(&mut project);
        store.save_observing_settings(&project).unwrap();
    };

    // A plan minimum of 30 degrees over the profile's 20 gives fewer hours.
    set(&|p| p.scheduling.minimum_altitude_degrees = Some(30.0));
    let raised = tonight(&a).await;
    assert_eq!(raised["limits"]["minimum_altitude_degrees"], 30.0);
    assert!(
        hours_up(&raised) < hours_up(&open) - 0.5,
        "{} then {}",
        hours_up(&open),
        hours_up(&raised)
    );

    // The custom horizon counts once the plan turns it on, raised by the
    // plan's offset in the drawn curve.
    set(&|p| {
        p.scheduling.minimum_altitude_degrees = Some(30.0);
        p.scheduling.use_custom_horizon = Some(true);
        p.scheduling.horizon_offset_degrees = Some(5.0);
    });
    let walled = tonight(&a).await;
    assert_eq!(walled["custom_horizon"], true);
    assert_eq!(walled["limits"]["horizon_offset_degrees"], 5.0);
    for sample in walled["curve"]["samples"].as_array().unwrap() {
        let target = &sample["targets"][0];
        let azimuth = target["azimuth_degrees"].as_f64().unwrap();
        let wall = if azimuth <= 180.0 {
            10.0 + 35.0 * azimuth / 180.0
        } else {
            45.0 - 35.0 * (azimuth - 180.0) / 180.0
        };
        let drawn = target["horizon_altitude_degrees"].as_f64().unwrap();
        assert!((drawn - wall - 5.0).abs() < 1e-6, "{azimuth}: {drawn}");
    }
    assert!(hours_up(&walled) <= hours_up(&raised));

    // An hour either side of the meridian is about two hours a night.
    set(&|p| p.scheduling.meridian_window_minutes = Some(60));
    let windowed = tonight(&a).await;
    assert_eq!(windowed["limits"]["meridian_window_minutes"], 60);
    assert!(
        hours_up(&windowed) > 1.8 && hours_up(&windowed) <= 2.1,
        "{}",
        hours_up(&windowed)
    );
}

/// Frames already accepted come off what a rig owes, panel by panel, and a
/// rig framed on a center of its own is timed there.
#[tokio::test]
async fn feasibility_owes_only_unaccepted_frames_and_times_a_rig_at_its_own_center() {
    let a = activated().await;
    place_in_los_angeles(&a, Value::Null, 25.0).await;
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let owed = |rig: &Value| rig["hours_needed"].as_f64().unwrap();
    // Six hours of 300 s frames on each of two panels, none taken yet.
    assert!((owed(&tonight(&a).await) - 12.0).abs() < 1e-6);

    let plans: Vec<i64> = {
        let mut statement =
            a.db.prepare("SELECT Id FROM exposureplan ORDER BY Id")
                .unwrap();
        statement
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    };
    assert_eq!(plans.len(), 2);
    let accept = |id: i64, frames: i64| {
        a.db.execute(
            "UPDATE exposureplan SET accepted=?2 WHERE Id=?1",
            [id, frames],
        )
        .unwrap();
    };
    // 24 frames of 300 s are two hours.
    accept(plans[0], 24);
    assert!((owed(&tonight(&a).await) - 10.0).abs() < 1e-6);
    // More than a panel asks for leaves that panel owing nothing, not less.
    accept(plans[1], 500);
    let rig = tonight(&a).await;
    assert!((owed(&rig) - 4.0).abs() < 1e-6, "{}", rig["hours_needed"]);
    assert_eq!(rig["center"]["dec_degrees"], 61.45);

    // Framed on its own far-southern center, the rig is timed there.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut framing = store.framing_draft(a.project).unwrap().unwrap();
        framing.rig_framings = vec![psf_guard_director_meta::framing::RigFraming {
            rig_id: a.rig,
            center: Some(psf_guard_director_core::visibility::IcrsPosition {
                ra_degrees: 38.2,
                dec_degrees: -75.0,
            }),
            position_angle_degrees: None,
            mosaic: psf_guard_director_core::framing::Mosaic {
                rows: 2,
                columns: 1,
                overlap_percent: 20,
            },
            panel: None,
        }];
        let revision = framing.revision;
        store.save_framing_draft(&framing, revision).unwrap();
    }
    let path = format!("/projects/{}/feasibility", a.project);
    let body = json!({"nights": 1, "start_ms": 1_790_294_400_000u64});
    let (status, view) = call(&a.f.app, "POST", &path, body, None).await;
    assert_eq!(status, StatusCode::OK, "{view}");
    // The shared center stays the view's; the rig has its own.
    assert_eq!(view["data"]["center"]["dec_degrees"], 61.45);
    let rig = &view["data"]["rigs"][0];
    assert_eq!(rig["center"]["dec_degrees"], -75.0);
    assert_eq!(hours_up(rig), 0.0);
    assert_eq!(rig["nights_to_complete"], Value::Null);
}
