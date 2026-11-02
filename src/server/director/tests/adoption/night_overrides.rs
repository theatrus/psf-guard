//! Phase 6's gates over HTTP: a Target Scheduler rig refuses overrides, a
//! Director rig's overrides reach the program it pulls and Director's
//! preview alike, and a night's overrides lapse after it.

use super::activation::{activated, Activated};
use super::*;
use psf_guard_director_core::{night::NightDate, windows::Interval};
use psf_guard_director_meta::night_override::{NightOverride, Subject};

/// 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
const START_MS: u64 = 1_790_294_400_000;
const LONGITUDE: f64 = -118.3;
const HOUR: u64 = 3_600_000;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

async fn activate(a: &Activated) {
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
}

/// Report the rig's equipment from Los Angeles and enable a workload
/// policy, which makes it a rig Director drives.
pub(super) async fn drive_with_director(a: &Activated) {
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let program: Value = serde_json::from_str(include_str!(
        "../../../../../crates/director-core/tests/fixtures/execution-program.json"
    ))
    .unwrap();
    let mut configuration = program["configuration"].clone();
    configuration["rig_id"] = json!(a.rig.to_string());
    configuration["filters"][0]["id"] = json!("filter-2");
    configuration["offset"] = json!({"support":"range","minimum":0,"maximum":100});
    let (status, reported) = call(&a.f.app, "PUT", &format!("/rigs/{}/equipment", a.rig), json!({
        "coordinator_instance_id": instance, "catalog_id": a.rig, "configuration": configuration,
        "filter_names": {"filter-2":"Ha"},
        "optics": {"sensor_width_px": 6248, "sensor_height_px": 4176, "pixel_size_um": 3.76, "focal_length_mm": 250.0, "aperture_mm": 51.0, "rotation": {"mode":"rotator"}},
        "site": {"latitude_degrees": 34.2, "longitude_degrees": LONGITUDE, "elevation_meters": 400.0},
        "horizon": null, "limits": null, "reported_at_ms": 1_700_000_000_000u64,
    }), None).await;
    assert_eq!(status, StatusCode::OK, "{reported}");
    // The runtime rule reads only whether the policy is on; commissioning
    // one through the API needs a paired plugin, which this fixture lacks.
    let policy = json!({
        "rig_id": a.rig, "catalog_id": a.rig, "client_id": Uuid::new_v4(), "profile_id": Uuid::new_v4(),
        "profile_revision": 1, "configuration_id": configuration["id"], "project_ids": [a.project],
        "enabled": true, "revision": 1,
    });
    Connection::open(a.f._dir.path().join("meta.sqlite"))
        .unwrap()
        .execute(
            "INSERT INTO workload_policy VALUES(?1,?2)",
            rusqlite::params![a.rig.to_string(), policy.to_string()],
        )
        .unwrap();
}

fn pull_path(a: &Activated) -> String {
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    format!(
        "/rigs/{}/program?coordinator_instance_id={instance}&catalog_id={}",
        a.rig, a.rig
    )
}

async fn pull(a: &Activated) -> Value {
    let (status, body) = call(&a.f.app, "GET", &pull_path(a), Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

fn overrides_path(a: &Activated, date: NightDate) -> String {
    format!("/rigs/{}/night-overrides/{date}", a.rig)
}

#[tokio::test]
async fn a_target_scheduler_rig_refuses_overrides_and_a_director_rig_checks_them() {
    let a = activated().await;
    activate(&a).await;
    let tonight = NightDate::at(now_ms(), LONGITUDE).unwrap();
    let hold = json!({"expected_revision": 0, "entries": [{"kind": "hold"}]});
    let (status, refused) = call(
        &a.f.app,
        "PUT",
        &overrides_path(&a, tonight),
        hold.clone(),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("Target Scheduler"));
    let (status, read) = call(
        &a.f.app,
        "GET",
        &overrides_path(&a, tonight),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["data"]["runtime"], "target_scheduler");
    assert_eq!(read["data"]["revision"], 0);
    // A draft is refused too, in place of a preview it could not shape.
    let draft = json!({"nights": 1, "start_ms": START_MS, "schedule_nights": 1,
        "override_draft": {"rig": a.rig, "date": "2026-09-24", "entries": [{"kind": "hold"}]}});
    let (status, _) = call(&a.f.app, "POST", "/night-plans", draft, None).await;
    assert_eq!(status, StatusCode::CONFLICT);

    drive_with_director(&a).await;
    let (status, stale) = call(
        &a.f.app,
        "PUT",
        &overrides_path(&a, tonight),
        json!({"expected_revision": 3, "entries": [{"kind": "hold"}]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{stale}");
    let (status, saved) = call(&a.f.app, "PUT", &overrides_path(&a, tonight), hold, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["runtime"], "director");
    assert_eq!(saved["data"]["entries"], json!([{"kind": "hold"}]));
    let revision = saved["data"]["revision"].as_u64().unwrap();
    assert!(revision > 0);
    let span = tonight.span(LONGITUDE);
    let outside = json!({"expected_revision": revision, "entries": [{"kind": "hold", "from_ms": span.end_ms + 1}]});
    let (status, refused) =
        call(&a.f.app, "PUT", &overrides_path(&a, tonight), outside, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused["error"]
        .as_str()
        .unwrap()
        .contains("inside the night"));
    let ended = NightDate::at(span.start_ms - 1, LONGITUDE).unwrap();
    let (status, refused) = call(
        &a.f.app,
        "PUT",
        &overrides_path(&a, ended),
        json!({"expected_revision": 0, "entries": [{"kind": "hold"}]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert!(refused["error"].as_str().unwrap().contains("has ended"));
    // Clearing returns the night to the rig's own program.
    let (status, cleared) = call(
        &a.f.app,
        "DELETE",
        &overrides_path(&a, tonight),
        json!({"expected_revision": revision}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["data"]["revision"], 0);
    assert_eq!(cleared["data"]["entries"], json!([]));
}

/// The pull carries a night's overrides as narrower windows, a night that
/// ended carries none, and clearing a night gives the rig back the program
/// it had.
#[tokio::test]
async fn overrides_reach_the_pulled_program_and_lapse_after_their_night() {
    let a = activated().await;
    activate(&a).await;
    drive_with_director(&a).await;
    let before = pull(&a).await;
    let revision = before["revision"].as_str().unwrap().to_owned();
    let goals = before["program"]["assignment"]["goals"]
        .as_array()
        .unwrap()
        .clone();
    assert_eq!(goals.len(), 2);
    let links = before["links"].as_array().unwrap();
    let target = |goal: &Value| {
        links
            .iter()
            .find(|link| link["goal_id"] == goal["id"])
            .unwrap()["target_guid"]
            .as_str()
            .unwrap()
            .parse::<Uuid>()
            .unwrap()
    };
    let (pinned, other) = (&goals[0], &goals[1]);

    // A night that has ended no longer reaches the program.
    let now = now_ms();
    let tonight = NightDate::at(now, LONGITUDE).unwrap();
    let ended = NightDate::at(tonight.span(LONGITUDE).start_ms - 1, LONGITUDE).unwrap();
    a.f.state
        .director
        .as_ref()
        .unwrap()
        .writer
        .lock()
        .unwrap()
        .save_night_overrides(
            a.rig,
            ended,
            &[NightOverride::Hold { from_ms: None }],
            0,
            now,
        )
        .unwrap();
    assert_eq!(pull(&a).await["revision"], revision.as_str());

    // A pin an hour from now, inside a night that has not ended.
    let (date, window) = if tonight.span(LONGITUDE).end_ms > now + 2 * HOUR {
        (tonight, (now + HOUR / 2, now + 3 * HOUR / 2))
    } else {
        let next = tonight.next().unwrap();
        let start = next.span(LONGITUDE).start_ms;
        (next, (start + 8 * HOUR, start + 10 * HOUR))
    };
    let pin = NightOverride::Pin {
        subject: Subject::Target {
            target_guid: target(pinned),
        },
        window: Interval {
            start_ms: window.0,
            end_ms: window.1,
        },
    };
    let (status, saved) = call(
        &a.f.app,
        "PUT",
        &overrides_path(&a, date),
        json!({"expected_revision": 0, "entries": [pin]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let narrowed = pull(&a).await;
    assert_ne!(narrowed["revision"], revision.as_str());
    let assignment = &narrowed["program"]["assignment"];
    let span = (
        assignment["valid_from_ms"].as_u64().unwrap(),
        assignment["expires_at_ms"].as_u64().unwrap(),
    );
    assert_eq!(span.1 - span.0, 24 * HOUR);
    let windows = |goal: &Value| -> Vec<(u64, u64)> {
        assignment["goals"]
            .as_array()
            .unwrap()
            .iter()
            .find(|g| g["id"] == goal["id"])
            .unwrap()["eligible_windows"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| {
                (
                    w["start_ms"].as_u64().unwrap(),
                    w["end_ms"].as_u64().unwrap(),
                )
            })
            .collect()
    };
    assert_eq!(windows(pinned), vec![span]);
    assert_eq!(
        windows(other),
        vec![(span.0, window.0), (window.1, span.1)],
        "the other panel gives the pinned window up"
    );

    // The rig ranks by its observing preferences, which a night's plan
    // order sets aside.
    let path = format!("/preferences/rig/{}", a.rig);
    let (_, preferences) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let mut settings = preferences["data"].clone();
    settings["enabled"] = true.into();
    let (status, enabled) = call(&a.f.app, "PUT", &path, settings, None).await;
    assert_eq!(status, StatusCode::OK, "{enabled}");
    assert!(!pull(&a).await["program"]["observing_preferences"].is_null());
    // A night's plan order lasts the night: the program ends with it.
    let order = json!({"expected_revision": saved["data"]["revision"], "entries": [pin, {"kind": "order", "plans": [a.project]}]});
    let (status, ordered) = call(&a.f.app, "PUT", &overrides_path(&a, date), order, None).await;
    assert_eq!(status, StatusCode::OK, "{ordered}");
    let capped = pull(&a).await;
    assert_eq!(
        capped["program"]["assignment"]["expires_at_ms"]
            .as_u64()
            .unwrap(),
        tonight.span(LONGITUDE).end_ms
    );
    // The night's order stands in place of the rig's observing
    // preferences, from that night on: an order for the next night leaves
    // the night under way to the preferences.
    let preferences = &capped["program"]["observing_preferences"];
    if date == tonight {
        assert!(preferences.is_null(), "{}", capped["program"]);
    } else {
        assert!(!preferences.is_null(), "{}", capped["program"]);
    }

    let (status, cleared) = call(
        &a.f.app,
        "DELETE",
        &overrides_path(&a, date),
        json!({"expected_revision": ordered["data"]["revision"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    // The rig keeps one issued program, so the cleared night's program is
    // a new issue of the program it had.
    let after = pull(&a).await;
    assert_ne!(after["revision"], narrowed["revision"]);
    let assignment = &after["program"]["assignment"];
    let span = (
        assignment["valid_from_ms"].as_u64().unwrap(),
        assignment["expires_at_ms"].as_u64().unwrap(),
    );
    assert_eq!(span.1 - span.0, 24 * HOUR);
    for goal in assignment["goals"].as_array().unwrap() {
        assert_eq!(
            goal["eligible_windows"],
            json!([{"start_ms": span.0, "end_ms": span.1}])
        );
    }
}

/// Director's preview applies the night's stored overrides, or an unsaved
/// draft in their place, through the same narrowing as the pull: inside a
/// pinned window only the pinned panel runs, a hold ends the night, and an
/// excluded panel gets no frames.
#[tokio::test]
async fn directors_preview_plans_a_night_with_its_overrides_or_a_draft() {
    let a = activated().await;
    activate(&a).await;
    drive_with_director(&a).await;
    let date: NightDate = "2026-09-24".parse().unwrap();
    let request = |draft: Option<Value>| {
        let mut body = json!({"rigs": [a.rig], "first_night": "2026-09-24", "nights": 1,
            "schedule_nights": 1, "plan_from_ms": START_MS});
        if let Some(draft) = draft {
            body["override_draft"] = json!({"rig": a.rig, "date": "2026-09-24", "entries": draft});
        }
        body
    };
    let plan = |body: Value| {
        let app = a.f.app.clone();
        async move {
            let (status, planned) = call(&app, "POST", "/night-plans", body, None).await;
            assert_eq!(status, StatusCode::OK, "{planned}");
            planned["data"]["rigs"][0].clone()
        }
    };
    let rig = plan(request(None)).await;
    assert_eq!(rig["runtime"], "director");
    let schedule = &rig["schedule"];
    assert_eq!(schedule["runtime"], "director", "{rig}");
    assert!(schedule["nights"][0]["overrides"].is_null());
    let goals = schedule["goals"].as_array().unwrap().clone();
    assert_eq!(goals.len(), 2, "{schedule}");
    let dusk = rig["nights"][0]["dusk_ms"].as_u64().unwrap();
    let dawn = rig["nights"][0]["dawn_ms"].as_u64().unwrap();
    // Each run's start, end and exposure plan.
    let runs_of = |night: &Value| -> Vec<(u64, u64, i64)> {
        night["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|b| b["runs"].as_array().unwrap().clone())
            .map(|r| {
                (
                    r["start_ms"].as_u64().unwrap(),
                    r["end_ms"].as_u64().unwrap(),
                    r["exposure_plan_id"].as_i64().unwrap(),
                )
            })
            .collect()
    };
    let window = (dusk + 2 * HOUR, dusk + 4 * HOUR);
    // Pin the panel the rig would not shoot in the window, so the pin
    // changes the night.
    let shot = runs_of(&schedule["nights"][0])
        .into_iter()
        .find(|(start, end, _)| *start < window.1 && *end > window.0)
        .map(|(_, _, plan)| plan)
        .unwrap();
    let (pinned, other) = if goals[0]["exposure_plan_id"].as_i64() == Some(shot) {
        (&goals[1], &goals[0])
    } else {
        (&goals[0], &goals[1])
    };

    let pin = json!([{"kind": "pin", "subject": {"kind": "target", "target_guid": pinned["target_guid"]},
        "window": {"start_ms": window.0, "end_ms": window.1}}]);
    let drafted = plan(request(Some(pin))).await;
    let night = &drafted["schedule"]["nights"][0];
    assert_eq!(night["overrides"]["draft"], true);
    assert!(night["overrides"]["revision"].is_null());
    let inside: Vec<i64> = runs_of(night)
        .into_iter()
        .filter(|(start, end, _)| *start < window.1 && *end > window.0)
        .map(|(_, _, plan)| plan)
        .collect();
    assert!(!inside.is_empty(), "{night}");
    assert!(inside
        .iter()
        .all(|plan| *plan == pinned["exposure_plan_id"].as_i64().unwrap()));

    let hold = json!([{"kind": "hold", "from_ms": dusk + HOUR}]);
    let held = plan(request(Some(hold))).await;
    let night = &held["schedule"]["nights"][0];
    assert!(runs_of(night).iter().all(|(_, end, _)| *end <= dusk + HOUR));
    assert!(
        night["gaps"]
            .as_array()
            .unwrap()
            .iter()
            .any(|g| g["reason"] == "held"
                && g["start_ms"].as_u64().unwrap() <= dusk + HOUR
                && g["end_ms"].as_u64().unwrap() >= dawn),
        "{night}"
    );

    // Stored overrides apply without a draft.
    let exclude = NightOverride::Exclude {
        subject: Subject::ExposurePlan {
            exposureplan_guid: other["exposureplan_guid"]
                .as_str()
                .unwrap()
                .parse()
                .unwrap(),
        },
        window: None,
    };
    let stored =
        a.f.state
            .director
            .as_ref()
            .unwrap()
            .writer
            .lock()
            .unwrap()
            .save_night_overrides(a.rig, date, &[exclude], 0, now_ms())
            .unwrap()
            .unwrap();
    let excluded = plan(request(None)).await;
    let night = &excluded["schedule"]["nights"][0];
    assert_eq!(night["overrides"]["revision"], stored.revision);
    assert_eq!(night["overrides"]["draft"], false);
    let runs = runs_of(night);
    assert!(!runs.is_empty());
    assert!(runs
        .iter()
        .all(|(_, _, plan)| *plan != other["exposure_plan_id"].as_i64().unwrap()));

    let outside = json!([{"kind": "hold", "from_ms": 1}]);
    let (status, refused) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        request(Some(outside)),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
}

/// A rig that stops counting as Director's pulls its program without the
/// overrides it had, and its nights can still be cleared, though no new
/// overrides are taken.
#[tokio::test]
async fn a_rig_no_longer_directors_drops_its_overrides_and_may_clear_them() {
    let a = activated().await;
    activate(&a).await;
    drive_with_director(&a).await;
    let now = now_ms();
    let tonight = NightDate::at(now, LONGITUDE).unwrap();
    let date = if tonight.span(LONGITUDE).end_ms > now + HOUR {
        tonight
    } else {
        tonight.next().unwrap()
    };
    let (status, saved) = call(
        &a.f.app,
        "PUT",
        &overrides_path(&a, date),
        json!({"expected_revision": 0, "entries": [{"kind": "hold"}]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let held = pull(&a).await;
    let span = date.span(LONGITUDE);
    let goals = |program: &Value| -> Vec<Vec<(u64, u64)>> {
        program["program"]["assignment"]["goals"]
            .as_array()
            .unwrap()
            .iter()
            .map(|goal| {
                goal["eligible_windows"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|w| {
                        (
                            w["start_ms"].as_u64().unwrap(),
                            w["end_ms"].as_u64().unwrap(),
                        )
                    })
                    .collect()
            })
            .collect()
    };
    // The hold takes the night from every goal.
    for windows in goals(&held) {
        assert!(
            windows
                .iter()
                .all(|(start, end)| *end <= span.start_ms || *start >= span.end_ms),
            "{windows:?}"
        );
    }

    Connection::open(a.f._dir.path().join("meta.sqlite"))
        .unwrap()
        .execute(
            "UPDATE workload_policy SET payload=json_set(payload,'$.enabled',json('false')) WHERE rig_id=?1",
            [a.rig.to_string()],
        )
        .unwrap();
    let plain = pull(&a).await;
    let assignment = &plain["program"]["assignment"];
    let whole = (
        assignment["valid_from_ms"].as_u64().unwrap(),
        assignment["expires_at_ms"].as_u64().unwrap(),
    );
    for windows in goals(&plain) {
        assert_eq!(windows, vec![whole]);
    }
    let (status, refused) = call(
        &a.f.app,
        "PUT",
        &overrides_path(&a, date),
        json!({"expected_revision": saved["data"]["revision"], "entries": [{"kind": "hold"}]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");
    let (status, cleared) = call(
        &a.f.app,
        "DELETE",
        &overrides_path(&a, date),
        json!({"expected_revision": saved["data"]["revision"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{cleared}");
    assert_eq!(cleared["data"]["entries"], json!([]));
}
