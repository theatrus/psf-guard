use super::activation::{activated, Activated};
use super::*;

/// 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
const START_MS: u64 = 1_790_294_400_000;

/// Give the rig a Los Angeles site with a rig minimum of 25°.
async fn place(a: &Activated) {
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
        "horizon": null,
        "sky_quality": null,
        "limits": {"value": {"minimum_altitude_degrees": 25.0, "maximum_altitude_degrees": 90.0, "meridian_exclusion": {"before_ms": 0, "after_ms": 0}}, "source": {"kind":"manual"}},
    });
    let (status, saved) = call(&a.f.app, "PUT", "/catalogs/rig/rig/profile", edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
}

/// A Target Scheduler project of the rig's profile with one target and one
/// H-alpha exposure plan on the template the fixture made.
fn project(a: &Activated, name: &str, minimum_altitude: f64, desired: u32, accepted: u32) -> i64 {
    a.db.execute(
        "INSERT INTO project (profileId, name, state, priority, minimumtime, minimumaltitude, usecustomhorizon,
            horizonoffset, meridianwindow, filterswitchfrequency, ditherevery, enablegrader, guid)
         VALUES ('profile-a', ?1, 1, 1, 30, ?2, 0, 0, 0, 0, 0, 1, lower(hex(randomblob(16))))",
        rusqlite::params![name, minimum_altitude],
    )
    .unwrap();
    let project = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid) VALUES (?1, 1, ?2, 61.45, 2, 0, 100, ?3)",
        rusqlite::params![format!("{name} target"), 38.2 / 15.0, project],
    )
    .unwrap();
    let target = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId)
         VALUES ('profile-a', 300, ?1, ?2, ?2, ?3, 1)",
        rusqlite::params![desired, accepted, target],
    )
    .unwrap();
    project
}

#[tokio::test]
async fn a_rigs_summary_counts_each_plans_hours_as_feasibility_does() {
    let a = activated().await;
    place(&a).await;
    let path = format!("/projects/{}/feasibility", a.project);
    let (status, feasibility) = call(
        &a.f.app,
        "POST",
        &path,
        json!({"nights": 3, "start_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{feasibility}");
    let rig = &feasibility["data"]["rigs"][0];
    let minimum = rig["limits"]["minimum_altitude_degrees"].as_f64().unwrap();
    // The same plan as Target Scheduler holds it once activated, under the
    // same limits, and one that is already done.
    let heart = project(&a, "Heart", minimum, 72, 0);
    project(&a, "Done", minimum, 10, 10);

    let (status, plans) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 3, "start_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{plans}");
    let data = &plans["data"];
    assert_eq!(data["nights"], 3);
    let rigs = data["rigs"].as_array().unwrap();
    assert_eq!(rigs.len(), 1, "{data}");
    let summary = &rigs[0];
    assert_eq!(summary["rig"]["id"], a.rig.to_string());
    assert_eq!(summary["runtime"], "target_scheduler");
    assert_eq!(summary["profile_id"], "profile-a");
    // Only the plan with frames still owed is listed: 72 frames of 300 s,
    // each with 15 s of overhead.
    let listed = summary["plans"].as_array().unwrap();
    assert_eq!(listed.len(), 1, "{summary}");
    assert_eq!(listed[0]["project_id"], heart);
    assert_eq!(listed[0]["targets"][0]["frames_owed"], 72);
    assert!((listed[0]["targets"][0]["hours_owed"].as_f64().unwrap() - 6.3).abs() < 1e-9);
    assert_eq!(listed[0]["limits"]["minimum_altitude_degrees"], minimum);

    let nights = summary["nights"].as_array().unwrap();
    let timed = rig["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 3);
    for (night, timed) in nights.iter().zip(timed) {
        assert_eq!(night["date"], timed["date"]);
        assert_eq!(night["dark_hours"], timed["dark_hours"]);
        assert_eq!(night["available_dark_hours"], timed["dark_hours"]);
        assert_eq!(night["moon_illumination"], timed["moon_illumination"]);
        assert_eq!(night["availability"]["state"], "available");
        let hours = &night["targets"][0];
        assert_eq!(
            hours["hours_up"], timed["targets"][0]["hours_up"],
            "{night}"
        );
        assert_eq!(hours["hours_up_available"], hours["hours_up"]);
        // The template has Moon avoidance off.
        assert_eq!(hours["hours_moon_blocked"], 0.0);
        let load = &night["load"];
        let demand = load["demand_hours"].as_f64().unwrap();
        assert!((demand - 6.3_f64.min(hours["hours_up"].as_f64().unwrap())).abs() < 1e-9);
        assert_eq!(load["available_hours"], night["dark_hours"]);
    }
    // Chosen rigs only; an unknown rig is listed in the warnings.
    let stranger = Uuid::new_v4();
    let (status, chosen) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"rigs": [stranger], "nights": 1, "start_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{chosen}");
    assert_eq!(chosen["data"]["rigs"], json!([]));
    assert!(chosen["data"]["warnings"][0]
        .as_str()
        .unwrap()
        .contains(&stranger.to_string()));
    // A first night by date is the same night for every rig, whatever the
    // instant; without one, each rig starts with the night under way there.
    let (status, dated) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 2, "first_night": "2026-09-26", "start_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{dated}");
    let nights = dated["data"]["rigs"][0]["nights"].as_array().unwrap();
    assert_eq!(
        (nights[0]["date"].as_str(), nights[1]["date"].as_str()),
        (Some("2026-09-26"), Some("2026-09-27"))
    );
    // A date not written YYYY-MM-DD is refused as the body is read.
    let (status, _) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"first_night": "2026-9-1"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    // 03:00 at the rig on the 25th is still the night of the 24th.
    let (_, small_hours) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS + 10 * 3_600_000}),
        None,
    )
    .await;
    assert_eq!(
        small_hours["data"]["rigs"][0]["nights"][0]["date"],
        "2026-09-24"
    );
    for body in [
        json!({"plan_from_ms": u64::MAX}),
        json!({"nights": 0}),
        json!({"nights": 32}),
        json!({"start_ms": 0}),
    ] {
        let (status, refused) = call(&a.f.app, "POST", "/night-plans", body, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }
}

#[tokio::test]
async fn marks_round_trip_and_clip_the_nights_they_cover() {
    let a = activated().await;
    place(&a).await;
    project(&a, "Heart", 25.0, 200, 0);
    let rig = a.rig;
    let (status, open) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 3, "start_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{open}");
    let nights = open["data"]["rigs"][0]["nights"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(open["data"]["rigs"][0]["mark_revisions"]["rig"], 0);
    // The 25th is out; the 26th is clear for two hours from an hour after dusk.
    let dusk = nights[2]["dusk_ms"].as_u64().unwrap();
    let window = json!({"start_ms": dusk + 3_600_000, "end_ms": dusk + 3 * 3_600_000});
    let marks = json!({"expected_revision": 0, "marks": [
        {"date": "2026-09-25", "state": "unavailable", "note": "dome motor"},
        {"date": "2026-09-26", "state": "limited", "windows": [window]},
    ]});
    let endpoint = format!("/night-availability/rig/{rig}");
    let (status, saved) = call(&a.f.app, "PUT", &endpoint, marks.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["revision"], 1);
    let (status, stale) = call(&a.f.app, "PUT", &endpoint, marks, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    let (status, read) = call(
        &a.f.app,
        "GET",
        &format!("/night-availability?scope=rig&id={rig}&from=2026-09-24&to=2026-09-30"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["data"]["marks"].as_array().unwrap().len(), 2);
    assert_eq!(read["data"]["marks"][0]["note"], "dome motor");

    let (_, marked) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 3, "start_ms": START_MS}),
        None,
    )
    .await;
    let summary = &marked["data"]["rigs"][0];
    assert_eq!(summary["mark_revisions"]["rig"], 1);
    let nights = summary["nights"].as_array().unwrap();
    assert_eq!(nights[0]["availability"]["state"], "available");
    assert_eq!(nights[0]["available_dark_hours"], nights[0]["dark_hours"]);
    assert_eq!(nights[1]["availability"]["state"], "unavailable");
    assert_eq!(nights[1]["availability"]["marks"][0]["note"], "dome motor");
    assert_eq!(nights[1]["available_dark_hours"], 0.0);
    assert_eq!(nights[1]["targets"][0]["hours_up_available"], 0.0);
    assert_eq!(nights[1]["load"]["demand_hours"], 0.0);
    assert_eq!(nights[2]["availability"]["state"], "limited");
    assert!(
        (nights[2]["available_dark_hours"].as_f64().unwrap() - 2.0).abs() < 1e-9,
        "{}",
        nights[2]
    );
    assert!(
        nights[2]["targets"][0]["hours_up_available"]
            .as_f64()
            .unwrap()
            <= 2.0 + 1e-9
    );
    assert!(nights[2]["load"]["demand_hours"].as_f64().unwrap() <= 2.0 + 1e-9);

    // Hours outside the night they are marked for are refused, and so is a
    // limited night with none.
    let outside = json!({"expected_revision": 1, "marks": [{"date": "2026-09-24", "state": "limited",
        "windows": [{"start_ms": dusk, "end_ms": dusk + 3_600_000}]}]});
    let (status, refused) = call(&a.f.app, "PUT", &endpoint, outside, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(
        refused["error"].as_str().unwrap().contains("2026-09-24"),
        "{refused}"
    );
    let empty =
        json!({"expected_revision": 1, "marks": [{"date": "2026-09-24", "state": "limited"}]});
    assert_eq!(
        call(&a.f.app, "PUT", &endpoint, empty, None).await.0,
        StatusCode::BAD_REQUEST
    );

    // Clearing returns the night to the default.
    let (status, cleared) = call(
        &a.f.app,
        "DELETE",
        &endpoint,
        json!({"expected_revision": 1, "dates": ["2026-09-25"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["data"]["revision"], 2);
    let (_, after) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 3, "start_ms": START_MS}),
        None,
    )
    .await;
    let nights = after["data"]["rigs"][0]["nights"].as_array().unwrap();
    assert_eq!(nights[1]["availability"]["state"], "available");
    assert_eq!(nights[2]["availability"]["state"], "limited");
    assert_eq!(
        call(
            &a.f.app,
            "PUT",
            &format!("/night-availability/cloud/{rig}"),
            json!({"expected_revision": 2, "marks": []}),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn a_target_scheduler_rig_gets_a_schedule_and_its_database_is_left_alone() {
    let a = activated().await;
    place(&a).await;
    project(&a, "Heart", 25.0, 400, 0);
    let path = a.f._dir.path().join("rig.sqlite");
    let before = std::fs::read(&path).unwrap();
    let body = json!({"nights": 2, "start_ms": START_MS, "schedule_nights": 2, "explain": "2026-09-24", "fidelity": "target_scheduler_preview", "plan_from_ms": START_MS});
    let (status, planned) = call(&a.f.app, "POST", "/night-plans", body, None).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    let schedule = &planned["data"]["rigs"][0]["schedule"];
    assert_eq!(schedule["runtime"], "target_scheduler");
    assert_eq!(schedule["assumptions"]["target_scheduler_version"], "5");
    assert_eq!(schedule["assumptions"]["acceptance"], "grader");
    let nights = schedule["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 2);
    assert_eq!(nights[0]["date"], "2026-09-24");
    let blocks = nights[0]["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty(), "{}", nights[0]);
    assert_eq!(blocks[0]["target"]["project_name"], "Heart");
    assert_eq!(blocks[0]["runs"][0]["filter"], "Ha");
    assert!(!blocks[0]["decision"]["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    // Only the explained night carries the planner's reasons.
    assert!(nights[1]["blocks"][0]["decision"]["candidates"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(nights[0]["gaps"][0]["reason"], "twilight");
    let planned_frames: u64 = schedule["progress"][0]["planned"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .sum();
    assert_eq!(
        planned_frames,
        nights[0]["frames"].as_u64().unwrap() + nights[1]["frames"].as_u64().unwrap()
    );
    assert!(planned_frames > 100);
    // A preview reads the rig's database and writes nothing to it.
    drop(planned);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // By default nothing is planned before now: these nights are past.
    let (_, past) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS, "schedule_nights": 1}),
        None,
    )
    .await;
    assert_eq!(
        past["data"]["rigs"][0]["schedule"]["nights"][0]["frames"],
        0
    );
    assert!(past["data"]["plan_from_ms"].as_u64().unwrap() > START_MS);
    // Without schedule_nights there is no schedule; more than the nights is refused.
    let (_, summary) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS}),
        None,
    )
    .await;
    assert!(summary["data"]["rigs"][0]["schedule"].is_null());
    let (status, _) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "schedule_nights": 2}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn a_director_schedule_runs_the_rigs_own_selection_over_the_pulled_program() {
    let a = activated().await;
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
    place(&a).await;
    let body = json!({"nights": 2, "start_ms": START_MS, "schedule_nights": 2, "runtime": "director", "plan_from_ms": START_MS});
    let (status, planned) = call(&a.f.app, "POST", "/night-plans", body.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    let rig = &planned["data"]["rigs"][0];
    // The rig runs Target Scheduler; the request asked for Director's view.
    assert_eq!(rig["runtime"], "target_scheduler");
    let schedule = &rig["schedule"];
    assert_eq!(schedule["runtime"], "director", "{rig}");
    assert!(schedule["assumptions"]["target_scheduler_version"].is_null());
    // No equipment report yet: the preview says what it assumed.
    assert!(
        rig["warnings"].as_array().unwrap().iter().any(|w| w
            .as_str()
            .unwrap()
            .contains("has not reported its equipment")),
        "{rig}"
    );
    let night = &schedule["nights"][0];
    let blocks = night["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty(), "{night}");
    assert_eq!(blocks[0]["decision"]["kind"], "ranked");
    assert!(blocks[0]["decision"]["note"]
        .as_str()
        .unwrap()
        .contains("priority"));
    assert_eq!(blocks[0]["runs"][0]["filter"], "Ha");
    assert!(blocks[0]["target"]["target_name"]
        .as_str()
        .unwrap()
        .starts_with("IC 1805"));
    // Both panels share the plan's six hours: 72 frames each, over two nights.
    let planned: u64 = schedule["progress"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|p| {
            p["planned"]
                .as_array()
                .unwrap()
                .iter()
                .map(|n| n.as_u64().unwrap())
        })
        .sum();
    assert!(planned > 60 && planned <= 144, "{planned}");
    assert!(night["gaps"]
        .as_array()
        .unwrap()
        .iter()
        .all(|g| g["end_ms"].as_u64() > g["start_ms"].as_u64()));
    // The pull still refuses the rig until it reports its equipment: the
    // preview's configuration never leaves the server.
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let (status, _) = call(
        &a.f.app,
        "GET",
        &format!(
            "/rigs/{}/program?coordinator_instance_id={instance}&catalog_id={}",
            a.rig, a.rig
        ),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
}
