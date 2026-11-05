use super::activation::{activated, Activated};
use super::*;
use crate::{config::ForecastsConfig, server::forecast::ForecastClient};
use psf_guard_director_core::priority::Scope;
use psf_guard_director_meta::site_forecast::ForecastFailure;
use std::sync::Mutex;

/// 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
const START_MS: u64 = 1_790_294_400_000;
const HOUR_MS: u64 = 3_600_000;

/// The forecast the stand-in answers with: each hour's start, cover and
/// dew point. The air is 10 °C throughout.
type Hours = Arc<Mutex<Vec<(u64, f64, f64)>>>;

/// An Open-Meteo stand-in that answers with the hours a test gives it.
async fn open_meteo(hours: Hours) -> String {
    let app = axum::Router::new()
        .route(
            "/v1/forecast",
            get(|State(hours): State<Hours>| async move {
                let hours = hours.lock().unwrap();
                Json(json!({"hourly": {
                    "time": hours.iter().map(|(start, _, _)| start / 1000).collect::<Vec<_>>(),
                    "cloud_cover": hours.iter().map(|(_, cover, _)| cover).collect::<Vec<_>>(),
                    "temperature_2m": hours.iter().map(|_| 10.0).collect::<Vec<_>>(),
                    "dew_point_2m": hours.iter().map(|(_, _, dew)| dew).collect::<Vec<_>>(),
                }}))
            }),
        )
        .with_state(hours);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{address}")
}

fn point_at(a: &Activated, url: &str) {
    a.f.state
        .set_forecast_client(ForecastClient::new(&ForecastsConfig {
            open_meteo_url: Some(url.into()),
            ..Default::default()
        }));
}

/// A Los Angeles site the rig plans at, with a Target Scheduler project.
async fn site(a: &Activated) -> Uuid {
    let site = Uuid::new_v4();
    let (status, _) = call(
        &a.f.app,
        "POST",
        "/sites",
        json!({"id": site, "name": "Backyard"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let location =
        json!({"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0});
    let edit = json!({"expected_revision": 0, "location": {"value": location, "source": {"kind": "manual"}}, "horizon": null});
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
    a.db.execute(
        "INSERT INTO project (profileId, name, state, priority, minimumtime, minimumaltitude, usecustomhorizon,
            horizonoffset, meridianwindow, filterswitchfrequency, ditherevery, enablegrader, guid)
         VALUES ('profile-a', 'Heart', 1, 1, 30, 25, 0, 0, 0, 0, 0, 1, lower(hex(randomblob(16))))",
        [],
    )
    .unwrap();
    let project = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid) VALUES ('Heart target', 1, ?1, 61.45, 2, 0, 100, ?2)",
        rusqlite::params![38.2 / 15.0, project],
    )
    .unwrap();
    let target = a.db.last_insert_rowid();
    a.db.execute(
        "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId)
         VALUES ('profile-a', 300, 200, 0, 0, ?1, 1)",
        [target],
    )
    .unwrap();
    site
}

async fn set_forecast(a: &Activated, site: Uuid, forecast: Value) -> Value {
    let path = format!("/sites/{site}/profile");
    let (_, read) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let profile = &read["data"]["profile"];
    let location = &profile["location"];
    let edit = json!({
        "expected_revision": profile["revision"],
        "location": {"value": location["value"], "source": location["source"]},
        "horizon": null,
        "forecast": forecast,
    });
    let (status, saved) = call(&a.f.app, "PUT", &path, edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    saved
}

async fn forecast(a: &Activated, site: Uuid) -> Value {
    let (status, view) = call(
        &a.f.app,
        "GET",
        &format!("/sites/{site}/forecast"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view["data"].clone()
}

async fn nights(a: &Activated, count: u32) -> Value {
    let (status, view) = call(
        &a.f.app,
        "POST",
        "/night-plans",
        json!({"nights": count, "start_ms": START_MS}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{view}");
    view["data"]["rigs"][0].clone()
}

#[tokio::test]
async fn a_sites_forecast_marks_its_nights_until_a_mark_or_its_expiry_says_otherwise() {
    let a = activated().await;
    place_rig(&a).await;
    let site = site(&a).await;

    // Turned on with the provider out of reach: the first ask fails, and
    // nothing is made up.
    point_at(&a, "http://127.0.0.1:9");
    set_forecast(
        &a,
        site,
        json!({"provider": "open_meteo", "cloud_threshold_percent": 30}),
    )
    .await;
    let mut view = forecast(&a, site).await;
    for _ in 0..500 {
        if !view["status"].is_null() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        view = forecast(&a, site).await;
    }
    assert_eq!(view["status"]["failures"], 1, "{view}");
    assert!(view["status"]["last_error"]
        .as_str()
        .unwrap()
        .starts_with("PSF Guard could not reach Open-Meteo"));
    assert_eq!(
        (view["forecast"].clone(), view["fresh"].clone()),
        (Value::Null, json!(false))
    );
    assert_eq!(
        view["attribution"],
        "Weather data by Open-Meteo.com (CC BY 4.0)"
    );
    let rig = nights(&a, 4).await;
    assert_eq!(rig["forecast"]["provider"], "open_meteo");
    assert_eq!(rig["forecast"]["fetched_at_ms"], Value::Null);
    assert!(rig["nights"]
        .as_array()
        .unwrap()
        .iter()
        .all(|n| n["availability"]["marks"].as_array().unwrap().is_empty()));

    // Three nights of forecast: cloudy in the afternoon but clear from an
    // hour before dusk, overcast, then clear for two hours from an hour
    // after dusk, the first of them a degree above the dew point. The
    // fourth is past its range.
    let first = rig["nights"].as_array().unwrap().clone();
    let span = |i: usize| first[i]["noon_ms"].as_u64().unwrap();
    let dusk = first[2]["dusk_ms"].as_u64().unwrap();
    let clear_from = (dusk / HOUR_MS + 1) * HOUR_MS;
    let evening = first[0]["dusk_ms"].as_u64().unwrap() - HOUR_MS;
    let hours: Vec<(u64, f64, f64)> = (span(0) / HOUR_MS * HOUR_MS..span(3) / HOUR_MS * HOUR_MS)
        .step_by(HOUR_MS as usize)
        .map(|start| {
            let cover = if start < span(1) {
                if start < evening {
                    90.0
                } else {
                    0.0
                }
            } else if start < span(2) {
                100.0
            } else if (clear_from..clear_from + 2 * HOUR_MS).contains(&start) {
                20.0
            } else {
                85.0
            };
            let dew = if start == clear_from { 9.0 } else { 0.0 };
            (start, cover, dew)
        })
        .collect();
    let url = open_meteo(Arc::new(Mutex::new(hours))).await;
    point_at(&a, &url);
    // Fetch now waits a minute after a failure, and a failure it makes
    // does not lengthen the timer's backoff.
    let refresh = format!("/sites/{site}/forecast/refresh");
    let (status, soon) = call(&a.f.app, "POST", &refresh, Value::Null, None).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{soon}");
    assert!(
        soon["error"]
            .as_str()
            .unwrap()
            .starts_with("The last request failed less than a minute ago."),
        "{soon}"
    );
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let failure = ForecastFailure {
            at_ms: now() - 2 * 60_000,
            error: "earlier".into(),
            retry_after_ms: None,
            from_timer: false,
        };
        assert_eq!(
            store
                .record_forecast_failure(site, &failure)
                .unwrap()
                .failures,
            1
        );
    }
    let (status, fetched) = call(&a.f.app, "POST", &refresh, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{fetched}");
    assert_eq!(
        (
            fetched["data"]["fresh"].clone(),
            fetched["data"]["status"]["failures"].clone()
        ),
        (json!(true), json!(0))
    );
    assert!(
        fetched["data"]["forecast"]["hours"]
            .as_array()
            .unwrap()
            .len()
            > 60
    );
    let (status, again) = call(
        &a.f.app,
        "POST",
        &format!("/sites/{site}/forecast/refresh"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{again}");

    let rig = nights(&a, 4).await;
    assert!(rig["forecast"]["fetched_at_ms"].is_u64());
    let nights_now = rig["nights"].as_array().unwrap();
    let state = |i: usize| {
        nights_now[i]["availability"]["state"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(
        (state(0), state(1), state(2), state(3)),
        (
            "available".into(),
            "unavailable".into(),
            "limited".into(),
            "available".into()
        )
    );
    let mark = &nights_now[1]["availability"]["marks"][0];
    assert_eq!(
        (
            mark["scope"].as_str(),
            mark["source"]["kind"].as_str(),
            mark["source"]["provider"].as_str()
        ),
        (Some("site"), Some("forecast"), Some("open_meteo"))
    );
    assert_eq!(nights_now[1]["available_dark_hours"], 0.0);
    assert!(
        (nights_now[2]["available_dark_hours"].as_f64().unwrap() - 2.0).abs() < 1e-9,
        "{}",
        nights_now[2]
    );
    // A night cloudy only in the afternoon is clear through its dark
    // hours, so it is available, and the forecast says why.
    assert_eq!(
        nights_now[0]["availability"]["marks"][0]["state"],
        "available"
    );
    assert_eq!(
        nights_now[0]["available_dark_hours"],
        nights_now[0]["dark_hours"]
    );
    assert!(!nights_now[2]["clouds"].as_array().unwrap().is_empty());
    assert!(nights_now[3]["clouds"].as_array().unwrap().is_empty());
    assert!(nights_now[3]["availability"]["marks"]
        .as_array()
        .unwrap()
        .is_empty());

    // A site mark wins over the forecast; a rig mark still meets it.
    let date = nights_now[1]["date"].as_str().unwrap().to_owned();
    let (status, saved) = call(&a.f.app, "PUT", &format!("/night-availability/site/{site}"),
        json!({"expected_revision": 0, "marks": [{"date": date, "state": "available", "note": "it cleared"}]}), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let rig_id = a.rig;
    let window = json!({"start_ms": clear_from + HOUR_MS, "end_ms": clear_from + 3 * HOUR_MS});
    let date2 = nights_now[2]["date"].as_str().unwrap().to_owned();
    let (status, saved) = call(&a.f.app, "PUT", &format!("/night-availability/rig/{rig_id}"),
        json!({"expected_revision": 0, "marks": [{"date": date2, "state": "limited", "windows": [window]}]}), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    let rig = nights(&a, 4).await;
    let marked = rig["nights"].as_array().unwrap();
    assert_eq!(marked[1]["availability"]["state"], "available");
    assert_eq!(
        marked[1]["availability"]["marks"][0]["source"]["kind"],
        "manual"
    );
    assert_eq!(marked[2]["availability"]["state"], "limited");
    assert!(
        (marked[2]["available_dark_hours"].as_f64().unwrap() - 1.0).abs() < 1e-9,
        "{}",
        marked[2]
    );

    // A looser threshold applies at once, with no new request.
    set_forecast(
        &a,
        site,
        json!({"provider": "open_meteo", "cloud_threshold_percent": 90}),
    )
    .await;
    assert_eq!(
        forecast(&a, site).await["forecast"]["fetched_at_ms"],
        fetched["data"]["forecast"]["fetched_at_ms"]
    );
    let rig = nights(&a, 4).await;
    assert_eq!(
        rig["nights"][2]["availability"]["marks"][0]["state"],
        "available"
    );
    assert_eq!(rig["forecast"]["dew_limit_celsius"], Value::Null);

    // A dew limit takes the hour a degree above the dew point out of the
    // clear hours, also with no new request.
    set_forecast(
        &a,
        site,
        json!({"provider": "open_meteo", "cloud_threshold_percent": 90, "dew_limit_celsius": 2}),
    )
    .await;
    let rig = nights(&a, 4).await;
    assert_eq!(rig["forecast"]["dew_limit_celsius"], 2);
    let mark = &rig["nights"][2]["availability"]["marks"][0];
    assert_eq!(mark["state"], "limited", "{mark}");
    let windows = mark["windows"].as_array().unwrap();
    assert!(
        windows.iter().all(|w| {
            let (start, end) = (
                w["start_ms"].as_u64().unwrap(),
                w["end_ms"].as_u64().unwrap(),
            );
            end <= clear_from || start >= clear_from + HOUR_MS
        }),
        "{mark}"
    );
    let dewy = rig["nights"][2]["clouds"]
        .as_array()
        .unwrap()
        .iter()
        .find(|hour| hour["start_ms"] == clear_from)
        .unwrap();
    assert_eq!(
        (
            dewy["temperature_celsius"].clone(),
            dewy["dew_point_celsius"].clone()
        ),
        (json!(10.0), json!(9.0))
    );
    set_forecast(
        &a,
        site,
        json!({"provider": "open_meteo", "cloud_threshold_percent": 90}),
    )
    .await;

    // A new elevation keeps the forecast; a move of a kilometre or more
    // discards it and requests the new place's.
    let kept_at = forecast(&a, site).await["forecast"]["fetched_at_ms"].clone();
    edit_site(
        &a,
        site,
        34.2,
        70.0,
        json!({"provider": "open_meteo", "cloud_threshold_percent": 90}),
    )
    .await;
    assert_eq!(
        forecast(&a, site).await["forecast"]["fetched_at_ms"],
        kept_at
    );
    edit_site(
        &a,
        site,
        34.3,
        70.0,
        json!({"provider": "open_meteo", "cloud_threshold_percent": 90}),
    )
    .await;
    let mut moved = forecast(&a, site).await;
    for _ in 0..500 {
        if moved["forecast"]["fetched_at_ms"].is_u64()
            && moved["forecast"]["fetched_at_ms"] != kept_at
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        moved = forecast(&a, site).await;
    }
    assert_ne!(moved["forecast"]["fetched_at_ms"], kept_at, "{moved}");
    // The timer refreshes a forecast three hours old.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut kept = store.site_forecast(site).unwrap().unwrap();
        store.clear_site_forecast(site).unwrap();
        kept.fetched_at_ms -= 4 * HOUR_MS;
        kept.expires_at_ms -= 4 * HOUR_MS;
        store.save_site_forecast(&kept).unwrap();
    }
    let old = forecast(&a, site).await["forecast"]["fetched_at_ms"]
        .as_u64()
        .unwrap();
    crate::server::director::site_forecast::refresh_due(&a.f.state).await;
    let renewed = forecast(&a, site).await["forecast"]["fetched_at_ms"]
        .as_u64()
        .unwrap();
    assert!(renewed > old + 3 * HOUR_MS, "{renewed} after {old}");

    // An expired forecast says nothing.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut kept = store.site_forecast(site).unwrap().unwrap();
        store.clear_site_forecast(site).unwrap();
        kept.fetched_at_ms -= 13 * HOUR_MS;
        kept.expires_at_ms -= 13 * HOUR_MS;
        store.save_site_forecast(&kept).unwrap();
    }
    let rig = nights(&a, 4).await;
    assert_eq!(rig["forecast"]["fetched_at_ms"], Value::Null);
    assert_eq!(rig["nights"][0]["availability"]["marks"], json!([]));
    assert_eq!(
        rig["nights"][1]["availability"]["marks"][0]["source"]["kind"],
        "manual"
    );
    assert_eq!(forecast(&a, site).await["fresh"], false);

    // Turned off, the kept forecast goes too.
    set_forecast(&a, site, Value::Null).await;
    let view = forecast(&a, site).await;
    assert_eq!(
        (
            view["forecast"].clone(),
            view["settings"].clone(),
            view["status"].clone()
        ),
        (Value::Null, Value::Null, Value::Null)
    );
    assert_eq!(nights(&a, 4).await["forecast"], Value::Null);
    let (status, refused) = call(
        &a.f.app,
        "POST",
        &format!("/sites/{site}/forecast/refresh"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{refused}");
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// Save the site's location and forecast, keeping its horizon.
async fn edit_site(a: &Activated, site: Uuid, latitude: f64, elevation: f64, forecast: Value) {
    let path = format!("/sites/{site}/profile");
    let (_, read) = call(&a.f.app, "GET", &path, Value::Null, None).await;
    let location = json!({"latitude_degrees": latitude, "longitude_degrees": -118.3, "elevation_meters": elevation});
    let edit = json!({
        "expected_revision": read["data"]["profile"]["revision"],
        "location": {"value": location, "source": {"kind": "manual"}},
        "horizon": null,
        "forecast": forecast,
    });
    let (status, saved) = call(&a.f.app, "PUT", &path, edit, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
}

/// Give the rig a Los Angeles location of its own and a 25° minimum.
async fn place_rig(a: &Activated) {
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

#[tokio::test]
async fn met_norway_s_not_modified_keeps_the_hours_and_when_they_arrived() {
    let a = activated().await;
    place_rig(&a).await;
    let site = site(&a).await;
    // A MET Norway stand-in: a forecast with its Last-Modified, and "not
    // modified" when that Last-Modified comes back verbatim.
    let since: Arc<Mutex<Vec<Option<String>>>> = Arc::default();
    let seen = Arc::clone(&since);
    let app = axum::Router::new().route(
        "/weatherapi/locationforecast/2.0/complete",
        get(move |headers: axum::http::HeaderMap| {
            let seen = Arc::clone(&seen);
            async move {
                let asked = headers
                    .get("if-modified-since")
                    .map(|v| v.to_str().unwrap().to_owned());
                seen.lock().unwrap().push(asked.clone());
                let mut answer = axum::http::HeaderMap::new();
                answer.insert("last-modified", "Sun, 01 Nov 2026 11:00:00 GMT".parse().unwrap());
                answer.insert("expires", "Sun, 01 Nov 2026 11:30:00 GMT".parse().unwrap());
                if asked.as_deref() == Some("Sun, 01 Nov 2026 11:00:00 GMT") {
                    return (StatusCode::NOT_MODIFIED, answer).into_response();
                }
                let body = json!({"type": "Feature", "properties": {"timeseries": [
                    {"time": "2026-11-01T12:00:00Z", "data": {"instant": {"details": {"cloud_area_fraction": 10.0, "air_temperature": 8.0, "dew_point_temperature": 2.0}}}},
                    {"time": "2026-11-01T13:00:00Z", "data": {"instant": {"details": {"cloud_area_fraction": 90.0, "air_temperature": 7.0, "dew_point_temperature": 2.0}}}},
                ]}});
                (answer, Json(body)).into_response()
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    a.f.state
        .set_forecast_client(ForecastClient::new(&ForecastsConfig {
            met_norway_url: Some(format!("http://{address}")),
            ..Default::default()
        }));

    set_forecast(
        &a,
        site,
        json!({"provider": "met_norway", "cloud_threshold_percent": 30}),
    )
    .await;
    let mut view = forecast(&a, site).await;
    for _ in 0..500 {
        if !view["forecast"].is_null() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        view = forecast(&a, site).await;
    }
    assert_eq!(
        view["forecast"]["last_modified"], "Sun, 01 Nov 2026 11:00:00 GMT",
        "{view}"
    );
    assert_eq!(view["forecast"]["checked_at_ms"], Value::Null);
    // Ten minutes on, Fetch now checks again.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut kept = store.site_forecast(site).unwrap().unwrap();
        store.clear_site_forecast(site).unwrap();
        kept.fetched_at_ms -= 10 * 60_000;
        kept.expires_at_ms -= 10 * 60_000;
        store.save_site_forecast(&kept).unwrap();
    }
    let before = forecast(&a, site).await["forecast"].clone();
    let (status, checked) = call(
        &a.f.app,
        "POST",
        &format!("/sites/{site}/forecast/refresh"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{checked}");
    let after = &checked["data"]["forecast"];
    assert_eq!(after["hours"], before["hours"]);
    assert_eq!(after["fetched_at_ms"], before["fetched_at_ms"]);
    let checked_at = after["checked_at_ms"].as_u64().unwrap();
    assert!(checked_at >= before["fetched_at_ms"].as_u64().unwrap() + 10 * 60_000);
    assert_eq!(
        after["expires_at_ms"].as_u64().unwrap(),
        checked_at + 12 * HOUR_MS
    );
    assert_eq!(checked["data"]["fresh"], true);
    assert_eq!(
        *since.lock().unwrap(),
        vec![None, Some("Sun, 01 Nov 2026 11:00:00 GMT".to_owned())]
    );
}
