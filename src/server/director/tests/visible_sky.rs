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

/// Give a rig that has its limits a Los Angeles location too.
async fn rig_profile_location(state: &Arc<AppState>, rig: Uuid) {
    let service = state.director.clone().unwrap();
    service
        .run(move |store| {
            let mut profile = store.rig_profile(rig)?.unwrap();
            let revision = profile.revision;
            profile.site = Some(Reported {
                value: Site {
                    latitude_degrees: 34.2,
                    longitude_degrees: -118.3,
                    elevation_meters: 400.0,
                },
                source: Source::Manual {},
                reported_at_ms: 0,
            });
            store.save_rig_profile(&profile, revision)
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
    // A date that is not a night, or a range past the last night the
    // astronomy times, is refused in a sentence the page can show.
    rig_profile_location(&state, rig).await;
    for (first, nights, reason) in [
        (
            "1970-01-01",
            1,
            "first_night must be a date from 1970-01-02 to 2098-12-31, written YYYY-MM-DD.",
        ),
        (
            "2099-01-01",
            1,
            "first_night must be a date from 1970-01-02 to 2098-12-31, written YYYY-MM-DD.",
        ),
        (
            "24 September",
            1,
            "first_night must be a date from 1970-01-02 to 2098-12-31, written YYYY-MM-DD.",
        ),
        (
            "2098-12-31",
            2,
            "The last night must be 2098-12-31 or earlier.",
        ),
    ] {
        let body = json!({"rig": rig, "first_night": first, "nights": nights});
        let (status, refused) = call(&app, "POST", PATH, body, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{first}: {refused}");
        assert_eq!(refused["error"], reason, "{first}");
    }
    // Nothing was measured for any of them.
    assert_eq!(state.director.as_ref().unwrap().visible_sky.computed().0, 0);
    // Both ends of the range are nights.
    for first in ["1970-01-02", "2098-12-31"] {
        let body = json!({"rig": rig, "first_night": first});
        let (status, view) = call(&app, "POST", PATH, body, None).await;
        assert_eq!(status, StatusCode::OK, "{first}: {view}");
        assert_eq!(view["data"]["nights"][0]["date"], first);
    }
}

#[tokio::test]
async fn requests_for_the_same_nights_measure_them_once_and_a_dropped_one_measures_nothing() {
    use super::super::visible_sky::{compute, nights_asked, read_inputs, Nights, Span, Wanted};
    use crate::concurrency::Stop;
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, true).await;
    let service = state.director.clone().unwrap();
    // A request whose caller has gone by its turn measures nothing.
    let inputs = service
        .clone()
        .with_reader(move |store| read_inputs(store, rig))
        .await
        .ok()
        .unwrap();
    let dates = nights_asked("2026-09-24", 2, 1).ok().unwrap();
    let gone = Stop::new();
    gone.request();
    assert!(compute(
        &state,
        &service,
        inputs,
        Nights::Dates(dates.clone()),
        30,
        Span::Now,
        Wanted::Picture { measured: false },
        crate::concurrency::Priority::Viewed,
        &gone
    )
    .is_err());
    assert_eq!(service.visible_sky.computed().0, 0);
    // Three requests at once for the same two nights: each night is
    // measured once, and all three get the same lines.
    let body = json!({"rig": rig, "first_night": "2026-09-24", "nights": 2});
    let (a, b, c) = tokio::join!(
        call(&app, "POST", PATH, body.clone(), None),
        call(&app, "POST", PATH, body.clone(), None),
        call(&app, "POST", PATH, body.clone(), None),
    );
    for (status, view) in [&a, &b, &c] {
        assert_eq!(*status, StatusCode::OK, "{view}");
    }
    assert_eq!(a.1["data"]["every_night"], b.1["data"]["every_night"]);
    assert_eq!(a.1["data"]["any_night"], c.1["data"]["any_night"]);
    assert_eq!(service.visible_sky.computed().0, 2);
    // The same inputs, not stopped, read the kept grids.
    let inputs = service
        .clone()
        .with_reader(move |store| read_inputs(store, rig))
        .await
        .ok()
        .unwrap();
    assert!(compute(
        &state,
        &service,
        inputs,
        Nights::Dates(dates),
        30,
        Span::Now,
        Wanted::Picture { measured: false },
        crate::concurrency::Priority::Interactive,
        &Stop::new()
    )
    .is_ok());
    assert_eq!(service.visible_sky.computed().0, 2);
    // The workers it leased are back.
    assert_eq!(
        state.leased_workers(crate::concurrency::Priority::Interactive),
        0
    );
    assert_eq!(
        state.leased_workers(crate::concurrency::Priority::Viewed),
        0
    );
    // A request with no priority is a page's own: the three requests the
    // route took held no interactive guard. The one a person asked for,
    // just now, did.
    assert_eq!(service.visible_sky.guards(), 1);
    let by_hand = json!({"rig": rig, "first_night": "2026-09-24", "nights": 2, "minimum_minutes": 45, "priority": "interactive"});
    let (status, body) = call(&app, "POST", PATH, by_hand, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(service.visible_sky.guards(), 2);
    // Nor does a count for a place, unless a person asked for it.
    let place = json!({"rig": rig, "first_night": "2026-09-24", "nights": 2, "places": [{"ra_degrees": 10.0, "dec_degrees": 80.0}]});
    let (status, body) = call(&app, "POST", "/sky/visible-places", place, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(service.visible_sky.guards(), 2);
    assert_eq!(state.interactive_job_name(), None);
    // Only the two priorities are taken.
    let other = json!({"rig": rig, "priority": "background"});
    let (status, _) = call(&app, "POST", PATH, other, None).await;
    assert!(status.is_client_error(), "{status}");
}

/// A page's own request waits while a person's work runs, and takes its
/// turn once that ends.
#[tokio::test]
async fn a_request_a_page_made_waits_for_a_persons_work() {
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, true).await;
    let service = state.director.clone().unwrap();
    let busy = state.begin_named_interactive_job("a stack someone started");
    let body = json!({"rig": rig, "first_night": "2026-09-24"});
    let waiting = tokio::spawn({
        let (app, body) = (app.clone(), body.clone());
        async move { call(&app, "POST", PATH, body, None).await }
    });
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    // Nothing is measured while the person's work runs.
    assert!(!waiting.is_finished());
    assert_eq!(service.visible_sky.computed().0, 0);
    // A person's own request for the sky does not wait behind it.
    let by_hand = json!({"rig": rig, "first_night": "2026-09-25", "priority": "interactive"});
    let (status, _) = call(&app, "POST", PATH, by_hand, None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(!waiting.is_finished());
    drop(busy);
    let (status, answer) = waiting.await.unwrap();
    assert_eq!(status, StatusCode::OK, "{answer}");
    assert_eq!(service.visible_sky.guards(), 1);
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
    assert_eq!(service.visible_sky.computed().0, 1);
    let (status, again) = call(&app, "POST", PATH, one.clone(), None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(again["data"]["every_night"], view["every_night"]);
    assert_eq!(service.visible_sky.computed().0, 1);
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
    assert_eq!(service.visible_sky.computed().0, 2);
    // Minutes that round up to the same samples are the same grid: 26
    // minutes needs six five-minute samples, as 30 does.
    let mut rounded = one.clone();
    rounded["minimum_minutes"] = json!(26);
    let (status, body) = call(&app, "POST", PATH, rounded, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["every_night"], view["every_night"]);
    assert_eq!(service.visible_sky.computed().0, 2);

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
    assert_eq!(service.visible_sky.computed().0, 4);
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
    assert_eq!(state.director.as_ref().unwrap().visible_sky.computed().0, 2);
}

/// A picture's counts unpacked: two bytes a node, low byte first.
fn unpacked(view: &Value) -> Vec<u16> {
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(view["counts"].as_str().unwrap())
        .unwrap();
    assert_eq!(bytes.len() % 2, 0);
    bytes
        .chunks(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect()
}

/// The count at the grid node nearest a place, from the packed counts.
fn count_at(view: &Value, ra: f64, dec: f64) -> u64 {
    let counts = unpacked(view);
    let (columns, rows) = (
        view["grid"]["columns"].as_u64().unwrap() as usize,
        view["grid"]["rows"].as_u64().unwrap() as usize,
    );
    let degrees = view["grid"]["degrees"].as_f64().unwrap();
    assert_eq!(counts.len(), columns * rows);
    let column = (ra / degrees).round() as usize % columns;
    let row = ((dec + 90.0) / degrees).round() as usize;
    u64::from(counts[row * columns + column])
}

#[tokio::test]
async fn the_month_and_the_year_count_the_nights_each_part_of_the_sky_is_in_view() {
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, true).await;
    let service = state.director.clone().unwrap();
    // Now: tonight and the 30 nights after.
    let now = json!({"rig": rig, "first_night": "2026-09-24", "span": "now"});
    let (status, body) = call(&app, "POST", PATH, now.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let month = body["data"].clone();
    assert_eq!(month["span"], "now");
    assert_eq!(month["every_nights"], 1);
    assert_eq!(month["nights"].as_array().unwrap().len(), 31);
    assert_eq!(month["nights"][30]["date"], "2026-10-24");
    assert_eq!(month["nights_counted"], 31);
    assert_eq!(
        month["grid"],
        json!({"degrees": 2.0, "columns": 180, "rows": 91})
    );
    assert_eq!(service.visible_sky.computed().0, 31);
    // The pole is in view every night, the far south on none; from 34.2°
    // north with a 25° minimum nothing south of about −31° ever is.
    assert_eq!(count_at(&month, 0.0, 90.0), 31);
    assert_eq!(count_at(&month, 200.0, 80.0), 31);
    for ra in [0.0, 90.0, 180.0, 270.0] {
        assert_eq!(count_at(&month, ra, -34.0), 0, "{ra}");
        assert_eq!(count_at(&month, ra, -90.0), 0, "{ra}");
    }
    // Sky by the Sun in late September is in view on no night of the
    // month, so the month does not reach every right ascension.
    assert_eq!(count_at(&month, 190.0, 0.0), 0);
    // The counts are the nights added up: a node counted on every night
    // is inside the every-night lines' share, one counted once or more
    // inside the any-night share.
    let counts = unpacked(&month);
    let share = |at_least: u16| {
        let (mut inside, mut whole) = (0.0, 0.0);
        for (row, nodes) in counts.chunks(180).enumerate() {
            let weight = (-90.0 + row as f64 * 2.0_f64).to_radians().cos();
            inside += weight * nodes.iter().filter(|count| **count >= at_least).count() as f64;
            whole += weight * 180.0;
        }
        (inside / whole * 10_000.0_f64).round() / 10_000.0
    };
    assert_eq!(
        share(31),
        month["every_night"]["sky_fraction"].as_f64().unwrap()
    );
    assert_eq!(
        share(1),
        month["any_night"]["sky_fraction"].as_f64().unwrap()
    );
    assert!(counts.iter().all(|count| *count <= 31));
    assert!(counts.iter().any(|count| (1..31).contains(count)));
    // The same nights one at a time add up to the same counts.
    let mut summed = vec![0_u16; counts.len()];
    for night in ["2026-09-24", "2026-10-09", "2026-10-24"] {
        let one = json!({"rig": rig, "first_night": night});
        let (status, body) = call(&app, "POST", PATH, one, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["data"]["nights_counted"], 1);
        let single = unpacked(&body["data"]);
        assert!(single.iter().all(|count| *count <= 1));
        for (sum, count) in summed.iter_mut().zip(&single) {
            *sum += count;
        }
        // A night of the month is read from what was kept.
        assert_eq!(service.visible_sky.computed().0, 31);
    }
    assert!(summed.iter().zip(&counts).all(|(three, all)| three <= all));
    assert!(summed
        .iter()
        .zip(&counts)
        .all(|(three, all)| *all < 31 || *three == 3));
    // Where the sky in view reaches: to the north pole, and south to the
    // line.
    let reach = &month["any_night"]["declinations"];
    assert_eq!(reach[1], 90.0);
    assert!(
        (-31.1..-28.5).contains(&reach[0].as_f64().unwrap()),
        "{reach}"
    );

    // Year: one night a week measured for a year, and the nights between
    // worked out. Its grids and skies are kept apart, and the month's are
    // all still kept.
    let year = json!({"rig": rig, "first_night": "2026-09-24", "span": "year"});
    let (status, body) = call(&app, "POST", PATH, year.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = body["data"].clone();
    assert_eq!(view["span"], "year");
    assert_eq!(view["every_nights"], 7);
    let nights = view["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 53);
    assert_eq!(nights[1]["date"], "2026-10-01");
    assert_eq!(nights[52]["date"], "2027-09-23");
    // Five of the year's nights are nights of the month, already
    // measured: the year reads those and measures the other 48.
    assert_eq!(service.visible_sky.computed(), (31, 48));
    // Every night from the first measured to the last is counted.
    assert_eq!(view["nights_counted"], 365);
    assert_eq!(count_at(&view, 0.0, 90.0), 365);
    assert_eq!(count_at(&view, 200.0, 80.0), 365);
    let year_counts = unpacked(&view);
    assert!(year_counts.iter().all(|count| *count <= 365));
    assert!(year_counts.iter().any(|count| (54..365).contains(count)));
    // The shares are the counts': every night, and any.
    let year_share = |at_least: u16| {
        let (mut inside, mut whole) = (0.0, 0.0);
        for (row, nodes) in year_counts.chunks(180).enumerate() {
            let weight = (-90.0 + row as f64 * 2.0_f64).to_radians().cos();
            inside += weight * nodes.iter().filter(|count| **count >= at_least).count() as f64;
            whole += weight * 180.0;
        }
        (inside / whole * 10_000.0_f64).round() / 10_000.0
    };
    assert_eq!(
        year_share(365),
        view["every_night"]["sky_fraction"].as_f64().unwrap()
    );
    assert_eq!(
        year_share(1),
        view["any_night"]["sky_fraction"].as_f64().unwrap()
    );
    // The measured nights' grids come with a year when asked for, for a
    // client to show one time of year from: a bit a node, 2048 bytes a
    // night. They are read from what the year kept: nothing is measured
    // again, and the rest of the answer is the same.
    assert!(month["measured"].is_null());
    assert!(view["measured"].is_null());
    let with_nights =
        json!({"rig": rig, "first_night": "2026-09-24", "span": "year", "measured": true});
    let (status, body) = call(&app, "POST", PATH, with_nights, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(service.visible_sky.computed(), (31, 48));
    let mut without = body["data"].clone();
    let measured = &body["data"]["measured"].clone();
    without.as_object_mut().unwrap().remove("measured");
    assert_eq!(without, view);
    // A month has no measured nights to send.
    let asked = json!({"rig": rig, "span": "now", "measured": true});
    let (status, body) = call(&app, "POST", PATH, asked, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert_eq!(
        body["error"],
        "Send measured only with span year: only a year has measured nights to send."
    );
    assert_eq!(measured["bytes_a_night"], 2048);
    assert_eq!(measured["turn_degrees_a_night"], 1.0);
    let bits = {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .decode(measured["in_view"].as_str().unwrap())
            .unwrap()
    };
    assert_eq!(bits.len(), 53 * 2048);
    let seen = |night: usize, index: usize| bits[night * 2048 + index / 8] >> (index % 8) & 1 == 1;
    // They are the grids the counts were made from. A measured night
    // asked for alone has the same grid.
    for (night, date) in [(0, "2026-09-24"), (1, "2026-10-01"), (30, "2027-04-22")] {
        let one = json!({"rig": rig, "first_night": date});
        let (status, body) = call(&app, "POST", PATH, one, None).await;
        assert_eq!(status, StatusCode::OK);
        let single = unpacked(&body["data"]);
        assert!(
            single
                .iter()
                .enumerate()
                .all(|(index, count)| (*count == 1) == seen(night, index)),
            "{date}"
        );
    }
    // Added up by the rule for the nights between, they give the year's
    // counts, and a month of them gives the reference's month.
    let grids: Vec<Vec<bool>> = (0..53)
        .map(|night| (0..180 * 91).map(|index| seen(night, index)).collect())
        .collect();
    let slices: Vec<&[bool]> = grids.iter().map(|grid| grid.as_slice()).collect();
    assert_eq!(
        super::super::visible_sky::year_window(&slices, 0, 365),
        year_counts
    );
    for first in [0, 98, 189, 334] {
        let window = super::super::visible_sky::year_window(&slices, first, 31);
        assert!(window.iter().all(|count| *count <= 31));
        assert!(window
            .iter()
            .zip(&year_counts)
            .all(|(month, year)| month <= year));
        // The pole every night, the far south none.
        assert_eq!(window[90 * 180], 31);
        assert_eq!(window[0], 0);
    }
    // The first month from the weekly nights against the month measured
    // night by night: never more nights at a node than were measured.
    let first = super::super::visible_sky::year_window(&slices, 0, 31);
    assert!(first
        .iter()
        .zip(&counts)
        .all(|(filled, exact)| *filled <= *exact + 2));
    // Most nodes agree, and those that differ lie along the edges.
    let differ = first
        .iter()
        .zip(&counts)
        .filter(|(filled, exact)| filled != exact)
        .count();
    assert!(differ < 180 * 91 / 10, "{differ}");

    // A place asked about for itself is counted as its node is in the
    // picture: a card and the map beside it agree.
    let nodes: Vec<(f64, f64)> = vec![(190.0, 0.0), (20.0, -20.0), (300.0, 40.0), (96.0, -28.0)];
    let places: Vec<Value> = nodes
        .iter()
        .map(|(ra, dec)| json!({"ra_degrees": ra, "dec_degrees": dec}))
        .collect();
    let ask = json!({"rig": rig, "first_night": "2026-09-24", "span": "year", "places": places});
    let (status, body) = call(&app, "POST", "/sky/visible-places", ask, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["nights_counted"], 365);
    for (place, (ra, dec)) in body["data"]["places"]
        .as_array()
        .unwrap()
        .iter()
        .zip(&nodes)
    {
        assert_eq!(
            place["nights_in_view"].as_u64().unwrap(),
            count_at(&view, *ra, *dec),
            "{ra} {dec}"
        );
    }
    // Over a year every right ascension has its season: north of the
    // limit all of it is in view on some night, and south of it none.
    for ra in (0..360).step_by(10) {
        let ra = f64::from(ra);
        assert!(count_at(&view, ra, -26.0) > 0, "{ra}");
        assert!(count_at(&view, ra, 0.0) > 0, "{ra}");
        assert_eq!(count_at(&view, ra, -34.0), 0, "{ra}");
    }
    let any = &view["any_night"];
    assert_eq!(any["declinations"][1], 90.0);
    assert!((-31.1..-28.5).contains(&any["declinations"][0].as_f64().unwrap()));
    // Its edge is one ring round the sky near −30°.
    for line in lines(any) {
        assert!(
            line.iter().all(|point| (-31.5..-27.0).contains(&point.1)),
            "{line:?}"
        );
    }
    assert!(any["sky_fraction"].as_f64() > month["any_night"]["sky_fraction"].as_f64());
    // The same year again measures nothing, and the month's nights are
    // still there to be read.
    let (status, _) = call(&app, "POST", PATH, year, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(service.visible_sky.computed(), (31, 48));
    let (status, _) = call(&app, "POST", PATH, now, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(service.visible_sky.computed().0, 31);
    // A year is not a count of nights, and nights in a row stop at 31.
    for (body, reason) in [
        (
            json!({"rig": rig, "span": "year", "nights": 10}),
            "Leave nights out with span year: a year always measures 53 nights, one a week.",
        ),
        (
            json!({"rig": rig, "span": "now", "nights": 32}),
            "nights must be between 1 and 31.",
        ),
        (
            json!({"rig": rig, "span": "year", "first_night": "2098-06-01"}),
            "The last night must be 2098-12-31 or earlier.",
        ),
    ] {
        let (status, refused) = call(&app, "POST", PATH, body, None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{refused}");
        assert_eq!(refused["error"], reason);
    }
}

#[tokio::test]
async fn a_place_is_counted_for_itself_and_agrees_with_the_lines() {
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, true).await;
    let service = state.director.clone().unwrap();
    const PLACES: &str = "/sky/visible-places";
    let nights = json!({"rig": rig, "first_night": "2026-09-24", "span": "now"});
    let (status, picture) = call(&app, "POST", PATH, nights.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{picture}");
    let picture = &picture["data"];
    // The southern limit, from the any-night line: about −30.6°.
    let limit = picture["any_night"]["declinations"][0].as_f64().unwrap();
    // Places a quarter of a degree either side of it, between grid nodes
    // at −30° and −32°: the nearest node would call both the same.
    let ask = |places: Value| {
        let mut body = nights.clone();
        body["places"] = places;
        body
    };
    let places = json!([
        {"ra_degrees": 0.0, "dec_degrees": 89.0},
        {"ra_degrees": 20.0, "dec_degrees": limit + 0.25},
        {"ra_degrees": 20.0, "dec_degrees": limit - 0.25},
        {"ra_degrees": 190.0, "dec_degrees": 0.0},
        {"ra_degrees": 20.0, "dec_degrees": -90.0},
    ]);
    let (status, body) = call(&app, "POST", PLACES, ask(places), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let view = &body["data"];
    assert_eq!(view["first_night"], "2026-09-24");
    assert_eq!(view["nights_counted"], 31);
    let counted: Vec<u64> = view["places"]
        .as_array()
        .unwrap()
        .iter()
        .map(|place| place["nights_in_view"].as_u64().unwrap())
        .collect();
    // The pole every night; just inside the line some nights and just
    // outside it none; by the Sun none; the south pole none.
    assert_eq!(counted[0], 31);
    assert!(counted[1] > 0, "{counted:?}");
    assert_eq!(counted[2..], [0, 0, 0]);
    assert_eq!(view["places"][1]["dec_degrees"], limit + 0.25);
    // Each agrees with a night counted alone for that place, and the
    // grids were not measured again for any of it.
    assert_eq!(service.visible_sky.computed(), (31, 0));
    // A year for a place, and the same nights without a picture first.
    let year = json!({"rig": rig, "first_night": "2026-09-24", "span": "year", "places": [{"ra_degrees": 190.0, "dec_degrees": 0.0}, {"ra_degrees": 0.0, "dec_degrees": 89.0}]});
    let (status, body) = call(&app, "POST", PLACES, year, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["nights_counted"], 365);
    let by_the_sun = body["data"]["places"][0]["nights_in_view"]
        .as_u64()
        .unwrap();
    assert!((140..365).contains(&by_the_sun), "{by_the_sun}");
    assert_eq!(body["data"]["places"][1]["nights_in_view"], 365);
    // Counting places measures no grid.
    assert_eq!(service.visible_sky.computed(), (31, 0));
    for (places, why) in [
        (json!([]), "none"),
        (json!([{"ra_degrees": 360.0, "dec_degrees": 0.0}]), "ra"),
        (json!([{"ra_degrees": 10.0, "dec_degrees": 91.0}]), "dec"),
        (
            Value::Array(vec![json!({"ra_degrees": 1.0, "dec_degrees": 1.0}); 17]),
            "too many",
        ),
    ] {
        let (status, refused) = call(&app, "POST", PLACES, ask(places), None).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{why}: {refused}");
        assert_eq!(
            refused["error"],
            "places must be 1 to 16 positions, each with ra_degrees from 0 up to 360 and dec_degrees from -90 to 90."
        );
    }
    // A rig with no site is told the same thing as for the picture.
    let (_dir, _state, app, _instance, _catalog, bare) = fixture().await;
    let body = json!({"rig": bare, "places": [{"ra_degrees": 1.0, "dec_degrees": 1.0}]});
    let (status, refused) = call(&app, "POST", PLACES, body, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        refused["error"],
        "Rig has no location. Choose a Planning site in Rig setup."
    );
}

#[tokio::test]
async fn a_request_with_no_date_starts_from_the_coming_night() {
    // The handler takes the clock, so this checks only that the nights it
    // counts begin today or tomorrow by the site's date and run on from
    // there; the rule itself is tested on fixed instants beside it.
    let (_dir, state, app, _instance, _catalog, rig) = fixture().await;
    rig_profile(&state, rig, true).await;
    let (status, body) = call(&app, "POST", PATH, json!({"rig": rig, "nights": 2}), None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let nights = body["data"]["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 2);
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    // The first night's dark has not ended.
    let dawn = nights[0]["dawn_ms"].as_u64().unwrap();
    assert!(dawn > now_ms, "{dawn} {now_ms}");
    // And it is the nearest such night: the one before had ended.
    assert!(dawn < now_ms + 2 * 86_400_000);
    let (status, places) = call(
        &app,
        "POST",
        "/sky/visible-places",
        json!({"rig": rig, "nights": 2, "places": [{"ra_degrees": 0.0, "dec_degrees": 89.0}]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{places}");
    assert_eq!(places["data"]["first_night"], nights[0]["date"]);
    assert_eq!(places["data"]["places"][0]["nights_in_view"], 2);
}
