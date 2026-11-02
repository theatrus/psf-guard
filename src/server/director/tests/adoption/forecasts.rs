use super::activation::{activated, Activated};
use super::*;
use crate::{config::ForecastsConfig, server::forecast::ForecastClient};
use psf_guard_director_core::priority::Scope;
use std::sync::Mutex;

/// 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
const START_MS: u64 = 1_790_294_400_000;
const HOUR_MS: u64 = 3_600_000;

/// The forecast the stand-in answers with: each hour's start and cover.
type Hours = Arc<Mutex<Vec<(u64, f64)>>>;

/// An Open-Meteo stand-in that answers with the hours a test gives it.
async fn open_meteo(hours: Hours) -> String {
    let app = axum::Router::new()
        .route(
            "/v1/forecast",
            get(|State(hours): State<Hours>| async move {
                let hours = hours.lock().unwrap();
                Json(json!({"hourly": {
                    "time": hours.iter().map(|(start, _)| start / 1000).collect::<Vec<_>>(),
                    "cloud_cover": hours.iter().map(|(_, cover)| cover).collect::<Vec<_>>(),
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

    // Three nights of forecast: clear, overcast, then clear for two hours
    // from an hour after dusk. The fourth is past its range.
    let first = rig["nights"].as_array().unwrap().clone();
    let span = |i: usize| first[i]["noon_ms"].as_u64().unwrap();
    let dusk = first[2]["dusk_ms"].as_u64().unwrap();
    let clear_from = (dusk / HOUR_MS + 1) * HOUR_MS;
    let hours: Vec<(u64, f64)> = (span(0) / HOUR_MS * HOUR_MS..span(3) / HOUR_MS * HOUR_MS)
        .step_by(HOUR_MS as usize)
        .map(|start| {
            let cover = if start < span(1) {
                0.0
            } else if start < span(2) {
                100.0
            } else if (clear_from..clear_from + 2 * HOUR_MS).contains(&start) {
                20.0
            } else {
                85.0
            };
            (start, cover)
        })
        .collect();
    let url = open_meteo(Arc::new(Mutex::new(hours))).await;
    point_at(&a, &url);
    let (status, fetched) = call(
        &a.f.app,
        "POST",
        &format!("/sites/{site}/forecast/refresh"),
        Value::Null,
        None,
    )
    .await;
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
    // A clear night still says the forecast is why.
    assert_eq!(
        nights_now[0]["availability"]["marks"][0]["state"],
        "available"
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

    // A looser threshold applies at once, with no new ask.
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
