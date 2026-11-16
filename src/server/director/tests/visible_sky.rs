use super::pairing::fixture;
use super::*;
use psf_guard_director_core::priority::Scope;
use psf_guard_director_core::visibility::Site;
use psf_guard_director_meta::profile::{Limits, Reported, RigProfile, Source};

const PATH: &str = "/sky/visible-region";

/// Give the rig a 25° minimum altitude and, with `placed`, a Los Angeles
/// location of its own.
async fn rig_profile(state: &Arc<AppState>, rig: Uuid, placed: bool) {
    let service = state.director.clone().unwrap();
    service
        .run(move |store| {
            let mut profile = RigProfile::empty(rig, 0);
            profile.limits = Reported {
                value: Limits {
                    minimum_altitude_degrees: 25.0,
                    ..Limits::default()
                },
                source: Source::Manual {},
                reported_at_ms: 0,
            };
            if placed {
                profile.site = Some(Reported {
                    value: Site {
                        latitude_degrees: 34.2,
                        longitude_degrees: -118.3,
                        elevation_meters: 400.0,
                    },
                    source: Source::Manual {},
                    reported_at_ms: 0,
                });
            }
            store.save_rig_profile(&profile, 0)
        })
        .await
        .unwrap();
}

fn lines(region: &Value) -> Vec<Vec<(f64, f64)>> {
    region["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| {
            line.as_array()
                .unwrap()
                .iter()
                .map(|point| (point[0].as_f64().unwrap(), point[1].as_f64().unwrap()))
                .collect()
        })
        .collect()
}

fn unit((ra, dec): (f64, f64)) -> [f64; 3] {
    let (ra, dec) = (ra.to_radians(), dec.to_radians());
    [dec.cos() * ra.cos(), dec.cos() * ra.sin(), dec.sin()]
}

#[tokio::test]
async fn a_rig_without_a_location_is_told_what_to_set() {
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    let (status, refused) = call(&app, "POST", PATH, json!({"rig": rig}), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
    assert_eq!(
        refused["error"],
        "Rig has no location. Choose a Planning site in Rig setup."
    );
    // Limits alone do not place it.
    rig_profile(&state, rig, false).await;
    let (status, _) = call(&app, "POST", PATH, json!({"rig": rig}), None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = call(&app, "POST", PATH, json!({"rig": Uuid::new_v4()}), None).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    for body in [
        json!({"rig": rig, "nights": 0}),
        json!({"rig": rig, "nights": 32}),
        json!({"rig": rig, "minimum_minutes": 1441}),
    ] {
        let (status, refused) = call(&app, "POST", PATH, body, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
    }
    // Nothing was measured for any of them.
    assert_eq!(state.director.as_ref().unwrap().visible_sky.computed(), 0);
}

#[tokio::test]
async fn one_night_is_one_set_of_lines_and_a_range_is_two() {
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, true).await;
    let service = state.director.clone().unwrap();
    let one = json!({"rig": rig, "first_night": "2026-09-24"});
    let (status, body) = call(&app, "POST", PATH, one.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = &body["data"];
    assert_eq!(view["rig"]["id"], rig.to_string());
    assert_eq!(view["location_from"], "rig");
    assert_eq!(view["horizon"], "flat");
    assert_eq!(view["horizon_from"], "none");
    assert_eq!(view["limits"]["minimum_altitude_degrees"], 25.0);
    // Target Scheduler's default minimum time, and the rig's own.
    assert_eq!(view["minimum_minutes"], 30);
    assert_eq!(view["rig_minimum_minutes"], 30);
    assert_eq!(view["nights"].as_array().unwrap().len(), 1);
    assert_eq!(view["nights"][0]["date"], "2026-09-24");
    assert!(view["nights"][0]["dusk_ms"].as_u64() < view["nights"][0]["dawn_ms"].as_u64());
    // One night: the two lines would be the same, so there is one.
    assert_eq!(view["any_night"], Value::Null);
    let every = lines(&view["every_night"]);
    assert!(!every.is_empty());
    for line in &every {
        assert_eq!(line.first(), line.last());
        assert!(line
            .iter()
            .all(|(ra, dec)| (0.0..360.0).contains(ra) && (-90.0..=90.0).contains(dec)));
    }
    // From 34.2° north with a 25° minimum, nothing south of −30.8° of
    // date is in view at all, and half an hour in view needs a little
    // more. The line is in ICRS, which precession has moved a seventh of
    // a degree by 2026, and is placed to an eighth of a degree.
    let south = every
        .iter()
        .flatten()
        .map(|point| point.1)
        .fold(f64::MAX, f64::min);
    assert!((-31.1..-28.5).contains(&south), "{south}");
    // The pole is 34.2° up all night: no line passes near it.
    let north = every
        .iter()
        .flatten()
        .map(|point| point.1)
        .fold(f64::MIN, f64::max);
    assert!(north < 80.0, "{north}");
    let share = view["every_night"]["sky_fraction"].as_f64().unwrap();
    assert!((0.4..0.75).contains(&share), "{share}");
    // The sky in view is on the left of the line: where the line is the
    // southern limit, that is north.
    let (line, at) = every
        .iter()
        .flat_map(|line| (1..line.len() - 1).map(move |at| (line, at)))
        .min_by(|a, b| a.0[a.1].1.total_cmp(&b.0[b.1].1))
        .unwrap();
    let (before, here, after) = (unit(line[at - 1]), unit(line[at]), unit(line[at + 1]));
    let along = [
        after[0] - before[0],
        after[1] - before[1],
        after[2] - before[2],
    ];
    let left_north = here[0] * along[1] - here[1] * along[0];
    assert!(left_north > 0.0, "{line:?}");

    // The night's grid is kept: the same request measures nothing, and a
    // longer minimum is another grid.
    assert_eq!(service.visible_sky.computed(), 1);
    let (status, again) = call(&app, "POST", PATH, one.clone(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["data"]["every_night"], view["every_night"]);
    assert_eq!(service.visible_sky.computed(), 1);
    let mut longer = one.clone();
    longer["minimum_minutes"] = json!(240);
    let (status, body) = call(&app, "POST", PATH, longer, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["minimum_minutes"], 240);
    assert_eq!(body["data"]["rig_minimum_minutes"], 30);
    let longer_share = body["data"]["every_night"]["sky_fraction"]
        .as_f64()
        .unwrap();
    assert!(longer_share < share - 0.05, "{longer_share} {share}");
    assert_eq!(service.visible_sky.computed(), 2);
    // Minutes that round up to the same samples are the same grid: 26
    // minutes needs six five-minute samples, as 30 does.
    let mut rounded = one.clone();
    rounded["minimum_minutes"] = json!(26);
    let (status, body) = call(&app, "POST", PATH, rounded, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["every_night"], view["every_night"]);
    assert_eq!(service.visible_sky.computed(), 2);

    // Three nights: the sky slides west under the dark hours, so less of
    // it is in view every night than on at least one.
    let mut range = one.clone();
    range["nights"] = json!(3);
    let (status, body) = call(&app, "POST", PATH, range, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = &body["data"];
    assert_eq!(view["nights"][2]["date"], "2026-09-26");
    let every_share = view["every_night"]["sky_fraction"].as_f64().unwrap();
    let any_share = view["any_night"]["sky_fraction"].as_f64().unwrap();
    assert!(every_share < any_share, "{every_share} {any_share}");
    assert!(every_share <= share && share <= any_share);
    assert!(!lines(&view["any_night"]).is_empty());
    assert_ne!(view["any_night"]["lines"], view["every_night"]["lines"]);
    // The first night was kept; two more were measured.
    assert_eq!(service.visible_sky.computed(), 4);
}

#[tokio::test]
async fn a_site_horizon_counts_whatever_a_plan_turns_on() {
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, false).await;
    // A site with a wall to the south, 70° tall from azimuth 150° to 210°,
    // and the rig planning there.
    let site = Uuid::new_v4();
    let (status, _) = call(
        &app,
        "POST",
        "/sites",
        json!({"id": site, "name": "Patio"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let point = |azimuth: f64, altitude: f64| json!({"azimuth_degrees": azimuth, "altitude_degrees": altitude});
    let edit = json!({
        "expected_revision": 0,
        "location": {"value": {"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0}, "source": {"kind": "manual"}},
        "horizon": {"value": {"mode": "custom", "points": [point(0.0, 0.0), point(149.0, 0.0), point(150.0, 70.0), point(210.0, 70.0), point(211.0, 0.0), point(360.0, 0.0)]}, "source": {"kind": "manual"}},
    });
    let (status, saved) = call(&app, "PUT", &format!("/sites/{site}/profile"), edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    {
        let mut store = state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut settings = store.observing_settings(Scope::Rig, rig).unwrap();
        settings.site_id = Some(site);
        store.save_observing_settings(&settings).unwrap();
    }
    let request = json!({"rig": rig, "first_night": "2026-09-24", "minimum_minutes": 60});
    let (status, body) = call(&app, "POST", PATH, request.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = &body["data"];
    assert_eq!(view["planning_site"]["name"], "Patio");
    assert_eq!(view["location_from"], "site");
    assert_eq!(view["horizon"], "custom");
    assert_eq!(view["horizon_from"], "site");
    // Custom horizon is off in the rig's Scheduling limits, as Target
    // Scheduler leaves it, and the wall still counts. Sky that crosses
    // the meridian low in the south is above 25° only near the meridian,
    // where the wall stands: what is left either side of it is under an
    // hour, so the southern limit moves north from −31° to about −18°.
    let south = lines(&view["every_night"])
        .iter()
        .flatten()
        .map(|point| point.1)
        .fold(f64::MAX, f64::min);
    assert!((-22.0..-14.0).contains(&south), "{south}");
    // Without the wall, the same hour reaches the flat limit again.
    let (status, saved) = call(
        &app,
        "PUT",
        &format!("/sites/{site}/profile"),
        json!({
            "expected_revision": 1,
            "location": {"value": {"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0}, "source": {"kind": "manual"}},
            "horizon": null,
        }),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let (status, body) = call(&app, "POST", PATH, request, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let open = &body["data"];
    assert_eq!(open["horizon"], "flat");
    assert_eq!(open["horizon_from"], "none");
    let open_south = lines(&open["every_night"])
        .iter()
        .flatten()
        .map(|point| point.1)
        .fold(f64::MAX, f64::min);
    assert!(open_south < -28.0, "{open_south}");
    assert!(
        open["every_night"]["sky_fraction"].as_f64() > view["every_night"]["sky_fraction"].as_f64()
    );
    // A changed horizon is a different sky: both were measured.
    assert_eq!(state.director.as_ref().unwrap().visible_sky.computed(), 2);
}
