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

#[tokio::test]
async fn a_plans_projection_follows_its_rigs_schedule_and_a_plan_ranked_ahead_delays_it() {
    let a = activated().await;
    let path = format!("/projects/{}/projection", a.project);
    let body = json!({"nights": 14, "start_ms": START_MS});
    // Before activation no rig's schedule has the plan.
    let (status, none) = call(&a.f.app, "POST", &path, body.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{none}");
    assert_eq!(none["data"]["rigs"], json!([]));
    assert!(none["data"]["warnings"][0]
        .as_str()
        .unwrap()
        .contains("not activated"));
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
    // The first night is clear for four hours only, so the plan needs most
    // of the second.
    let (_, open) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS}),
        None,
    )
    .await;
    let dusk = open["data"]["rigs"][0]["nights"][0]["dusk_ms"]
        .as_u64()
        .unwrap();
    let window = json!({"start_ms": dusk + 3_600_000, "end_ms": dusk + 5 * 3_600_000});
    let (status, marked) = call(&a.f.app, "PUT", &format!("/night-availability/rig/{}", a.rig),
        json!({"expected_revision": 0, "marks": [{"date": "2026-09-24", "state": "limited", "windows": [window]}]}), None).await;
    assert_eq!(status, StatusCode::OK, "{marked}");

    // Alone on the rig, the plan completes on the night its planned frames
    // reach what it owes.
    let (status, alone) = call(&a.f.app, "POST", &path, body.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{alone}");
    let rig = &alone["data"]["rigs"][0];
    assert_eq!(rig["runtime"], "target_scheduler", "{alone}");
    assert_eq!(rig["outcome"]["kind"], "scheduled", "{rig}");
    assert_eq!(rig["outcome"]["unscheduled"], 0);
    assert_eq!(rig["others"], json!([]));
    let planned: Vec<u64> = rig["planned"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_u64().unwrap())
        .collect();
    let owed = planned.iter().sum::<u64>();
    // Six hours of H-alpha at 300 s on each of two panels.
    assert_eq!(owed, 144, "{planned:?}");
    let last = planned.iter().rposition(|&n| n > 0).unwrap();
    let alone_on = rig["outcome"]["completes_on"].as_str().unwrap().to_owned();
    assert_eq!(alone_on, rig["dates"][last].as_str().unwrap());

    // Lowered to low priority, with a high-priority plan on the same sky
    // owing 25 hours, the plan gets the rig later, and completes later.
    a.db.execute(
        "UPDATE project SET priority = 0 WHERE name = 'Heart Nebula'",
        [],
    )
    .unwrap();
    a.db.execute(
        "INSERT INTO project (profileId, name, state, priority, minimumtime, minimumaltitude, usecustomhorizon,
            horizonoffset, meridianwindow, filterswitchfrequency, ditherevery, enablegrader, guid)
         VALUES ('profile-a', 'Soul first', 1, 2, 30, 25, 0, 0, 0, 0, 0, 1, lower(hex(randomblob(16))))",
        [],
    )
    .unwrap();
    let soul = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid) VALUES ('IC 1848', 1, ?1, 61.0, 2, 0, 100, ?2)",
        rusqlite::params![38.2 / 15.0, soul],
    )
    .unwrap();
    let target = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId)
         VALUES ('profile-a', 300, 300, 0, 0, ?1, 1)",
        [target],
    )
    .unwrap();
    let (status, shared) = call(&a.f.app, "POST", &path, body.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{shared}");
    let rig = &shared["data"]["rigs"][0];
    assert_eq!(rig["others"][0]["name"], "Soul first", "{rig}");
    assert!(rig["others"][0]["hours"].as_f64().unwrap() > 1.0, "{rig}");
    let later = rig["outcome"]["completes_on"].as_str().unwrap();
    assert!(later > alone_on.as_str(), "{later} after {alone_on}");
    assert_eq!(rig["planned"][0], 0, "{rig}");

    // Director ranks only the plans activated on the rig, so the
    // Target Scheduler project does not hold this plan up there.
    let mut director = body.clone();
    director["runtime"] = json!("director");
    let (status, ranked) = call(&a.f.app, "POST", &path, director, None).await;
    assert_eq!(status, StatusCode::OK, "{ranked}");
    let rig = &ranked["data"]["rigs"][0];
    assert_eq!(rig["runtime"], "director", "{rig}");
    assert_eq!(rig["outcome"]["kind"], "scheduled", "{rig}");
    assert_eq!(rig["outcome"]["completes_on"], json!(alone_on), "{rig}");
    assert_eq!(rig["others"], json!([]));
    // The schedule's notes come along: here, the equipment it assumed.
    assert!(
        rig["notes"].as_array().unwrap().iter().any(|n| n
            .as_str()
            .unwrap()
            .contains("has not reported its equipment")),
        "{rig}"
    );

    // A project set inactive by hand is not scheduled; activating again
    // would not change that, and the reason says what would.
    a.db.execute(
        "UPDATE project SET state = 2 WHERE name = 'Heart Nebula'",
        [],
    )
    .unwrap();
    let (_, inactive) = call(&a.f.app, "POST", &path, body.clone(), None).await;
    let rig = &inactive["data"]["rigs"][0];
    assert_eq!(rig["outcome"]["kind"], "skipped", "{rig}");
    assert!(
        rig["outcome"]["reason"]
            .as_str()
            .unwrap()
            .contains("Set it active in Target Scheduler"),
        "{rig}"
    );

    // Every frame taken: done, on either runtime.
    a.db.execute(
        "UPDATE project SET state = 1 WHERE name = 'Heart Nebula'",
        [],
    )
    .unwrap();
    a.db.execute(
        "UPDATE exposureplan SET accepted = desired, acquired = 2 * desired
         WHERE targetid IN (SELECT t.Id FROM target t JOIN project p ON p.Id = t.projectid WHERE p.name = 'Heart Nebula')",
        [],
    )
    .unwrap();
    for runtime in ["target_scheduler", "director"] {
        let mut done = body.clone();
        done["runtime"] = json!(runtime);
        let (_, finished) = call(&a.f.app, "POST", &path, done, None).await;
        assert_eq!(
            finished["data"]["rigs"][0]["outcome"],
            json!({"kind": "done"}),
            "{runtime}: {finished}"
        );
    }
}

/// A Target Scheduler rig's night is planned with unsaved changes to its
/// projects and targets, as Target Scheduler would read them, and the
/// preview writes nothing. The controls read the rows as saved.
#[tokio::test]
async fn a_target_scheduler_draft_plans_the_rig_as_changed_and_writes_nothing() {
    let a = activated().await;
    place(&a).await;
    let heart = project(&a, "Heart", 25.0, 400, 0);
    let soul = project(&a, "Soul", 25.0, 400, 0);
    let path = a.f._dir.path().join("rig.sqlite");
    let before = std::fs::read(&path).unwrap();
    let body = |draft: Option<Value>| {
        let mut body = json!({"nights": 1, "start_ms": START_MS, "schedule_nights": 1, "plan_from_ms": START_MS});
        if let Some(changes) = draft {
            body["ts_draft"] = json!({"rig": a.rig, "changes": changes});
        }
        body
    };
    let shot = |planned: &Value| -> Vec<String> {
        planned["data"]["rigs"][0]["schedule"]["nights"][0]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["target"]["project_name"].as_str().unwrap().to_owned())
            .collect()
    };
    let (status, planned) = call(&a.f.app, "POST", "/night-plans", body(None), None).await;
    assert_eq!(status, StatusCode::OK, "{planned}");
    let rig = &planned["data"]["rigs"][0];
    assert_eq!(rig["ts_save"]["files"], json!([path.to_string_lossy()]));
    assert!(rig["ts_save"]["refused"].is_null());
    let projects = rig["ts_projects"].as_array().unwrap();
    assert!(projects
        .iter()
        .any(|p| p["name"] == "Heart" && p["state"] == "active" && p["priority"] == "normal"));
    assert!(shot(&planned).iter().any(|name| name == "Heart"), "{rig}");

    // With Heart set inactive, Target Scheduler would shoot only Soul.
    let (status, drafted) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        body(Some(
            json!({"projects": [{"project_id": heart, "state": "inactive"}]}),
        )),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{drafted}");
    let names = shot(&drafted);
    assert!(
        !names.is_empty() && names.iter().all(|name| name == "Soul"),
        "{names:?}"
    );
    // The controls still read Heart as saved: active.
    assert!(drafted["data"]["rigs"][0]["ts_projects"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["project_id"] == heart && p["state"] == "active"));
    // A change to a row the database lacks is named, not planned, and
    // the changes after it are still planned.
    let (_, missing) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        body(Some(json!({
            "projects": [{"project_id": 999, "state": "inactive"}, {"project_id": heart, "state": "inactive"}],
            "targets": [{"target_id": 999, "active": false}],
        }))),
        None,
    )
    .await;
    let warnings = missing["data"]["rigs"][0]["warnings"].as_array().unwrap();
    for named in ["Project 999 is no longer", "Target 999 is no longer"] {
        assert!(
            warnings.iter().any(|w| w.as_str().unwrap().contains(named)),
            "{missing}"
        );
    }
    let names = shot(&missing);
    assert!(
        !names.is_empty() && names.iter().all(|name| name == "Soul"),
        "{names:?}"
    );
    let _ = soul;
    drop((planned, drafted, missing));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

/// Saving writes the changed columns of each row only while the row holds
/// what the person read, in the rig's database and, for a catalog kept in
/// step with Target Scheduler, in Target Scheduler's file, all or nothing.
#[tokio::test]
async fn saving_target_scheduler_changes_writes_rows_that_still_hold_what_was_read() {
    let a = activated().await;
    place(&a).await;
    let heart = project(&a, "Heart", 25.0, 400, 0);
    a.db.execute(
        "UPDATE target SET guid = lower(hex(randomblob(16))) WHERE guid IS NULL",
        [],
    )
    .unwrap();
    let target: i64 =
        a.db.query_row(
            "SELECT Id FROM target WHERE projectid = ?1",
            [heart],
            |row| row.get(0),
        )
        .unwrap();
    let path = format!("/rigs/{}/target-scheduler", a.rig);
    // Heart's project and target, their ids moved on by `offset`.
    let row_at = |db: &Connection, offset: i64| -> (i64, i64, bool) {
        let (priority, state): (i64, i64) = db
            .query_row(
                "SELECT priority, state FROM project WHERE Id = ?1",
                [heart + offset],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        let active: bool = db
            .query_row(
                "SELECT active FROM target WHERE Id = ?1",
                [target + offset],
                |r| r.get(0),
            )
            .unwrap();
        (priority, state, active)
    };
    let row = |db: &Connection| row_at(db, 0);
    let change = json!({
        "projects": [{"project_id": heart, "priority": "high", "was": {"priority": "normal", "state": "active"}}],
        "targets": [{"target_id": target, "active": false, "was_active": true}],
    });
    let (status, saved) = call(&a.f.app, "PUT", &path, change.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        (
            saved["data"]["projects"].as_u64(),
            saved["data"]["targets"].as_u64()
        ),
        (Some(1), Some(1))
    );
    assert_eq!(row(&a.db), (2, 1, false));
    // The same change again: the rows no longer hold what it read.
    let (status, stale) = call(&a.f.app, "PUT", &path, change, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    assert!(stale["error"]
        .as_str()
        .unwrap()
        .contains("changed in the rig's database"));
    let (status, _) = call(
        &a.f.app,
        "PUT",
        &path,
        json!({"projects": [{"project_id": heart, "priority": "low"}]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);

    // Kept in step with Target Scheduler: its file is written too, where
    // Target Scheduler numbers the same rows, found by GUID, its own way.
    let rig_db = a.f._dir.path().join("rig.sqlite");
    let source = keep_in_step(&a);
    let ts = Connection::open(&source).unwrap();
    ts.execute_batch(
        "PRAGMA foreign_keys = OFF;
         UPDATE project SET Id = Id + 1000;
         UPDATE target SET Id = Id + 1000, projectid = projectid + 1000;
         UPDATE exposureplan SET targetid = targetid + 1000;",
    )
    .unwrap();
    let back = json!({"projects": [{"project_id": heart, "priority": "normal", "was": {"priority": "high", "state": "active"}}]});
    let (status, saved) = call(&a.f.app, "PUT", &path, back, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(
        saved["data"]["files"],
        json!([rig_db.to_string_lossy(), source.to_string_lossy()])
    );
    assert_eq!(row(&a.db).0, 1);
    assert_eq!(row_at(&ts, 1000), (1, 1, false));
    // N.I.N.A. changes the row in Target Scheduler's file: nothing is
    // written, in either file.
    ts.execute(
        "UPDATE project SET priority = 0 WHERE Id = ?1",
        [heart + 1000],
    )
    .unwrap();
    let again = json!({"projects": [{"project_id": heart, "priority": "high", "was": {"priority": "normal", "state": "active"}}]});
    let (status, refused) = call(&a.f.app, "PUT", &path, again, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("Target Scheduler's database"));
    assert_eq!(row(&a.db).0, 1, "the rig's database is left as it was");
    assert_eq!(row_at(&ts, 1000).0, 0);
}

/// Keep the rig's database in step with a copy of it as Target
/// Scheduler's own file, and return that file.
fn keep_in_step(a: &Activated) -> std::path::PathBuf {
    let rig_db = a.f._dir.path().join("rig.sqlite");
    let source = a.f._dir.path().join("TargetScheduler.sqlite");
    std::fs::copy(&rig_db, &source).unwrap();
    let mut context = crate::server::database_context::DatabaseContext::new(
        "rig".into(),
        "RedCat rig".into(),
        rig_db.to_string_lossy().into(),
        vec![a.f._dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        a.f._dir.path().join("cache"),
    )
    .unwrap();
    context.ts_sync = Some(crate::ts_sync::Settings {
        source_path: source.to_string_lossy().into(),
        writeback: false,
        on_open: false,
        on_change: false,
        interval_minutes: 0,
    });
    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(context));
    source
}

/// Saving writes only rows of the rig's N.I.N.A. profile, finds a row in
/// Target Scheduler's file only by a GUID it has, and needs a server that
/// may write into rig databases.
#[tokio::test]
async fn target_scheduler_changes_reach_only_the_rigs_profile_and_rows_with_guids() {
    let a = activated().await;
    place(&a).await;
    let heart = project(&a, "Heart", 25.0, 400, 0);
    let soul = project(&a, "Soul", 25.0, 400, 0);
    let other = project(&a, "Other", 25.0, 400, 0);
    a.db.execute(
        "UPDATE project SET profileId = 'profile-b' WHERE Id = ?1",
        [other],
    )
    .unwrap();
    let path = format!("/rigs/{}/target-scheduler", a.rig);
    let raise = |project: i64| json!({"projects": [{"project_id": project, "priority": "high", "was": {"priority": "normal", "state": "active"}}]});
    let priority = |db: &Connection, project: i64| -> i64 {
        db.query_row(
            "SELECT priority FROM project WHERE Id = ?1",
            [project],
            |r| r.get(0),
        )
        .unwrap()
    };
    // Another profile's project is not the rig's.
    let (status, refused) = call(&a.f.app, "PUT", &path, raise(other), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("no longer in the rig's database"));
    assert_eq!(priority(&a.db, other), 1);

    // Rows from before Target Scheduler's GUIDs hold an empty one, which
    // would match every such row in its file, the other profile's too.
    let source = keep_in_step(&a);
    let ts = Connection::open(&source).unwrap();
    for db in [&a.db, &ts] {
        db.execute(
            "UPDATE project SET guid = '' WHERE Id IN (?1, ?2)",
            [heart, other],
        )
        .unwrap();
    }
    let (status, refused) = call(&a.f.app, "PUT", &path, raise(heart), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("has no GUID"));
    for db in [&a.db, &ts] {
        assert_eq!((priority(db, heart), priority(db, other)), (1, 1));
    }

    // A server that may not write into rig databases says so, and refuses.
    a.f.state.set_allow_database_management(false);
    let (_, planned) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS}),
        None,
    )
    .await;
    assert!(planned["data"]["rigs"][0]["ts_save"]["refused"]
        .as_str()
        .unwrap()
        .contains("needs database management"));
    let (status, _) = call(&a.f.app, "PUT", &path, raise(soul), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(priority(&a.db, soul), 1);
}

/// A rig Director drives, or one that runs on another PSF Guard, refuses
/// Target Scheduler changes.
#[tokio::test]
async fn target_scheduler_changes_are_refused_where_they_would_not_decide_or_reach_the_rig() {
    let a = activated().await;
    place(&a).await;
    let heart = project(&a, "Heart", 25.0, 400, 0);
    let path = format!("/rigs/{}/target-scheduler", a.rig);
    let change = json!({"projects": [{"project_id": heart, "priority": "high", "was": {"priority": "normal", "state": "active"}}]});
    // A rig whose database lives on a Sync peer.
    let set_peer = |peer: Option<&str>| {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut profile = store.rig_profile(a.rig).unwrap().unwrap();
        let revision = profile.revision;
        profile.peer_id = peer.map(str::to_owned);
        store.save_rig_profile(&profile, revision).unwrap();
    };
    set_peer(Some("observatory"));
    let (status, refused) = call(&a.f.app, "PUT", &path, change.clone(), None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("another PSF Guard (observatory)"));
    let (_, planned) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS}),
        None,
    )
    .await;
    assert!(planned["data"]["rigs"][0]["ts_save"]["refused"]
        .as_str()
        .unwrap()
        .contains("observatory"));
    set_peer(None);

    super::night_overrides::drive_with_director(&a).await;
    let (status, refused) = call(&a.f.app, "PUT", &path, change, None).await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("Director picks this rig's targets"));
}
