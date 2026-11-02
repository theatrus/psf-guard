use super::activation::{activated, Activated};
use super::*;
use psf_guard_director_core::priority::Scope;

/// 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
const START_MS: u64 = 1_790_294_400_000;

/// A patio's horizon, as N.I.N.A. writes it: a 70° wall all round but for
/// an opening to the south-east, down to 20° from azimuth 100° to 200°.
const PATIO: &str = "# Patio\n0 70\n99 70\n100 20\n200 20\n201 70\n360 70\n";

/// A site in Los Angeles with the patio's horizon, read from its `.hrz`
/// text, and the rig planning there with no location or horizon of its own.
async fn patio(a: &Activated, hrz: &str) -> Value {
    let (status, horizon) = call(
        &a.f.app,
        "POST",
        "/horizons/parse",
        json!({"text": hrz}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{horizon}");
    let site = Uuid::new_v4();
    let (status, _) = call(
        &a.f.app,
        "POST",
        "/sites",
        json!({"id": site, "name": "Patio"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let location =
        json!({"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0});
    let edit = json!({
        "expected_revision": 0,
        "location": {"value": location, "source": {"kind": "manual"}},
        "horizon": {"value": horizon["data"], "source": {"kind": "manual"}},
    });
    let (status, saved) = call(
        &a.f.app,
        "PUT",
        &format!("/sites/{site}/profile"),
        edit,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut rig = store.observing_settings(Scope::Rig, a.rig).unwrap();
        rig.site_id = Some(site);
        store.save_observing_settings(&rig).unwrap();
    }
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
        "site": null,
        "horizon": null,
        "sky_quality": null,
        "limits": {"value": {"minimum_altitude_degrees": 15.0, "maximum_altitude_degrees": 90.0, "meridian_exclusion": {"before_ms": 0, "after_ms": 0}}, "source": {"kind":"manual"}},
    });
    let (status, saved) = call(&a.f.app, "PUT", "/catalogs/rig/rig/profile", edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    horizon["data"].clone()
}

/// A Target Scheduler project with one target and one exposure plan, on
/// the rig's custom horizon or on its fixed minimum.
fn project(a: &Activated, name: &str, ra_hours: f64, dec: f64, custom_horizon: bool) -> i64 {
    a.db.execute(
        "INSERT INTO project (profileId, name, state, priority, minimumtime, minimumaltitude, usecustomhorizon,
            horizonoffset, meridianwindow, filterswitchfrequency, ditherevery, enablegrader, guid)
         VALUES ('profile-a', ?1, 1, 1, 30, 15, ?2, 0, 0, 0, 0, 1, lower(hex(randomblob(16))))",
        rusqlite::params![name, custom_horizon],
    )
    .unwrap();
    let project = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid) VALUES (?1, 1, ?2, ?3, 2, 0, 100, ?4)",
        rusqlite::params![format!("{name} target"), ra_hours, dec, project],
    )
    .unwrap();
    let target = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId)
         VALUES ('profile-a', 300, 200, 0, 0, ?1, 1)",
        [target],
    )
    .unwrap();
    project
}

/// Each project's dark hours up on the first night, and the frames the
/// rig's Target Scheduler schedule plans for it.
async fn hours_and_frames(a: &Activated) -> (Value, BTreeMap<i64, (f64, u64)>) {
    let (status, view) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": 1, "start_ms": START_MS, "schedule_nights": 1, "plan_from_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    let rig = view["data"]["rigs"][0].clone();
    let mut found = BTreeMap::new();
    for target in rig["nights"][0]["targets"].as_array().unwrap() {
        let frames: u64 = rig["schedule"]["progress"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|p| p["project_id"] == target["project_id"])
            .map(|p| p["planned"][0].as_u64().unwrap())
            .sum();
        found.insert(
            target["project_id"].as_i64().unwrap(),
            (target["hours_up"].as_f64().unwrap(), frames),
        );
    }
    (rig, found)
}

#[tokio::test]
async fn a_sites_hrz_horizon_hides_what_is_behind_it_from_plans_that_use_it() {
    let a = activated().await;
    let horizon = patio(&a, PATIO).await;
    assert_eq!(horizon["mode"], "custom");
    // The Heart rides in the north, never above 64° here: behind the wall
    // all night on the custom horizon, up for hours on the fixed minimum.
    let walled = project(&a, "Heart behind the wall", 2.5467, 61.45, true);
    let open = project(&a, "Heart on the minimum", 2.5467, 61.45, false);
    // Orion climbs through the south-east opening before dawn.
    let orion = project(&a, "Orion in the opening", 5.588, -5.39, true);
    let orion_open = project(&a, "Orion on the minimum", 5.588, -5.39, false);

    let (rig, found) = hours_and_frames(&a).await;
    assert_eq!(rig["warnings"], json!([]), "{rig}");
    let plan = |id: i64| {
        rig["plans"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["project_id"] == id)
            .unwrap()
            .clone()
    };
    assert_eq!(plan(walled)["limits"]["custom_horizon"], true);
    assert_eq!(plan(open)["limits"]["custom_horizon"], false);
    assert_eq!(found[&walled], (0.0, 0), "{found:?}");
    assert!(found[&open].0 > 4.0, "{found:?}");
    assert!(found[&open].1 > 0, "{found:?}");
    // The opening lets Orion through for part of the hours the fixed
    // minimum gives it.
    assert!(found[&orion].0 > 0.5, "{found:?}");
    assert!(found[&orion].0 < found[&orion_open].0, "{found:?}");
    // No block of the schedule lands on the walled Heart.
    for night in rig["schedule"]["nights"].as_array().unwrap() {
        for block in night["blocks"].as_array().unwrap() {
            assert_ne!(
                block["target"]["project_id"].as_i64(),
                Some(walled),
                "{block}"
            );
        }
    }
}

#[tokio::test]
async fn directors_schedule_keeps_out_of_what_the_sites_horizon_hides() {
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
    let director = |a: &Activated| {
        let app = a.f.app.clone();
        async move {
            let body = json!({"nights": 1, "start_ms": START_MS, "schedule_nights": 1, "runtime": "director", "plan_from_ms": START_MS});
            let (status, planned) = call(&app, "POST", "/night-plans", body, None).await;
            assert_eq!(status, StatusCode::OK, "{planned}");
            let schedule = planned["data"]["rigs"][0]["schedule"].clone();
            assert_eq!(schedule["runtime"], "director", "{planned}");
            schedule["nights"][0].clone()
        }
    };
    // The plan's Heart panels ride in the north, behind the patio's wall:
    // Director, which applies the rig's horizon to every goal, plans nothing.
    patio(&a, PATIO).await;
    let walled = director(&a).await;
    assert_eq!(walled["blocks"], json!([]), "{walled}");
    // A low horizon at a new site leaves them up for most of the night.
    patio(&a, "0 10\n360 10\n").await;
    let open = director(&a).await;
    assert!(!open["blocks"].as_array().unwrap().is_empty(), "{open}");
}
