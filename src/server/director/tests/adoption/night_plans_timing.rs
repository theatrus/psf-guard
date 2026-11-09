//! How long night plans take at a realistic size: four Target Scheduler
//! rigs at two sites with custom horizons and cloud forecasts, a dozen
//! projects on each, with three targets of four or five exposure plans
//! each. Run it with
//!
//! ```text
//! cargo test --release night_plans_timing -- --ignored --nocapture
//! ```
//!
//! It prints the wall time of each request shape the pages send, computed
//! afresh each time: first with nothing kept between requests, then with
//! the skies kept from the request before. With `NIGHT_PLANS_DUMP` set to
//! a folder, it writes each shape's response there, to compare one build's
//! answers with another's. With `NIGHT_PLANS_PHASES` set, it prints the
//! debug lines that time each phase of each computation.

use super::*;
use psf_guard_director_core::priority::Scope;
use psf_guard_director_meta::site_forecast::{ForecastHour, ForecastProvider, SiteForecast};

/// 2026-09-25 00:00 UTC, 17:00 the day before in Los Angeles.
const START_MS: u64 = 1_790_294_400_000;
const HOUR_MS: u64 = 3_600_000;
const RIGS: usize = 4;
const PROJECTS: usize = 12;
const TARGETS: usize = 3;
/// A horizon with a wall to the north and trees to the west.
const HORIZON: &str = "0 35\n45 30\n90 15\n135 12\n180 10\n225 18\n270 28\n315 40\n360 35\n";

/// Filters and whether each avoids the Moon.
const FILTERS: [(&str, bool); 7] = [
    ("L", true),
    ("R", true),
    ("G", true),
    ("B", true),
    ("Ha", true),
    ("OIII", true),
    ("SII", false),
];

pub(super) struct Rigs {
    pub(super) state: Arc<AppState>,
    pub(super) app: Router,
    _dir: TempDir,
}

/// A fixed GUID, so two builds' answers can be compared.
fn guid(kind: u128, n: usize) -> String {
    Uuid::from_u128((kind << 64) | n as u128).to_string()
}

/// One rig database with a dozen projects of three targets each.
fn rig_database(path: &std::path::Path, rig: usize) {
    let db = crate::ts_schema::create_fresh_db(path).unwrap();
    for (index, (filter, moon)) in FILTERS.iter().enumerate() {
        db.execute(
            "INSERT INTO exposuretemplate (profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
                moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
                moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
             VALUES ('profile-a', ?1, ?2, 100, 30, 1, -1, 0, ?3, ?4, 7, 0, 300, 0, 5, -15, 0, -1, 0, ?5)",
            rusqlite::params![
                format!("{filter} 300"),
                filter,
                i32::from(*moon),
                if index < 4 { 100.0 } else { 40.0 },
                guid(1, rig * 100 + index)
            ],
        )
        .unwrap();
    }
    for project in 0..PROJECTS {
        let seed = rig * PROJECTS + project;
        db.execute(
            "INSERT INTO project (profileId, name, state, priority, minimumtime, minimumaltitude, usecustomhorizon,
                horizonoffset, meridianwindow, filterswitchfrequency, ditherevery, enablegrader, guid)
             VALUES ('profile-a', ?1, 1, ?2, 30, ?3, ?4, ?5, ?6, 0, 1, 1, ?7)",
            rusqlite::params![
                format!("Project {seed}"),
                (seed % 3) as i64,
                [20.0, 25.0, 30.0][seed % 3],
                i32::from(seed.is_multiple_of(2)),
                [0.0, 5.0][seed % 2],
                [0, 0, 120][seed % 3],
                guid(2, seed)
            ],
        )
        .unwrap();
        let project_id = db.last_insert_rowid();
        for target in 0..TARGETS {
            let n = seed * TARGETS + target;
            let ra_degrees = (n as f64 * 37.3) % 360.0;
            let dec = -15.0 + (n as f64 * 13.7) % 80.0;
            db.execute(
                "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid) VALUES (?1, 1, ?2, ?3, 2, 0, 100, ?4, ?5)",
                rusqlite::params![format!("Target {n}"), ra_degrees / 15.0, dec, project_id, guid(3, n)],
            )
            .unwrap();
            let target_id = db.last_insert_rowid();
            // Narrowband or broadband, with some frames already taken.
            let templates: &[i64] = if n.is_multiple_of(2) {
                &[5, 6, 7]
            } else {
                &[1, 2, 3, 4, 5]
            };
            for (k, template) in templates.iter().enumerate() {
                let desired = 40 + ((n + k) % 5) as i64 * 30;
                let accepted = ((n * 7 + k * 3) % 20) as i64;
                db.execute(
                    "INSERT INTO exposureplan (profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, guid)
                     VALUES ('profile-a', ?1, ?2, ?3, ?3, ?4, ?5, ?6)",
                    rusqlite::params![
                        [300.0, 180.0, 120.0][(n + k) % 3],
                        desired,
                        accepted,
                        target_id,
                        template,
                        guid(4, n * 10 + k)
                    ],
                )
                .unwrap();
            }
        }
    }
}

/// A site with the horizon above and a forecast over the first fortnight.
async fn site(rigs: &Rigs, name: &str, location: Value) -> Uuid {
    let (status, horizon) = call(
        &rigs.app,
        "POST",
        "/horizons/parse",
        json!({"text": HORIZON}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{horizon}");
    let site = Uuid::parse_str(&guid(5, name.len())).unwrap();
    let (status, _) = call(
        &rigs.app,
        "POST",
        "/sites",
        json!({"id": site, "name": name}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let edit = json!({
        "expected_revision": 0,
        "location": {"value": location, "source": {"kind": "manual"}},
        "horizon": {"value": horizon["data"], "source": {"kind": "manual"}},
        "forecast": {"provider": "open_meteo", "cloud_threshold_percent": 30, "dew_limit_celsius": 2},
    });
    let (status, saved) = call(
        &rigs.app,
        "PUT",
        &format!("/sites/{site}/profile"),
        edit,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    // Fetched before the first night and good for decades, so every run
    // plans with it.
    let fetched_at_ms = START_MS - 24 * HOUR_MS;
    let expires_at_ms = 4_000_000_000_000;
    let first = START_MS / HOUR_MS * HOUR_MS - 12 * HOUR_MS;
    let hours = (0..16 * 24)
        .map(|hour| {
            let start_ms = first + hour * HOUR_MS;
            ForecastHour {
                start_ms,
                end_ms: start_ms + HOUR_MS,
                cloud_total: [0.0, 10.0, 60.0, 95.0, 20.0][(hour as usize / 5) % 5],
                cloud_low: None,
                cloud_mid: None,
                cloud_high: None,
                temperature_celsius: Some(10.0),
                dew_point_celsius: Some([2.0, 9.0][(hour as usize / 7) % 2]),
                humidity_percent: None,
            }
        })
        .collect();
    let forecast = SiteForecast {
        site_id: site,
        provider: ForecastProvider::OpenMeteo,
        fetched_at_ms,
        checked_at_ms: None,
        expires_at_ms,
        refresh_after_ms: Some(expires_at_ms),
        last_modified: None,
        hours,
    };
    let mut store = rigs.state.director.as_ref().unwrap().writer.lock().unwrap();
    store.save_site_forecast(&forecast).unwrap();
    site
}

/// Four rigs, two at each of two sites.
pub(super) async fn rigs() -> Rigs {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    // No forecast is fetched: the stored ones stand.
    state.set_forecast_client(crate::server::forecast::ForecastClient::new(
        &crate::config::ForecastsConfig {
            open_meteo_url: Some("http://127.0.0.1:9".into()),
            ..Default::default()
        },
    ));
    let app = router(state.clone());
    let rigs = Rigs {
        state,
        app,
        _dir: dir,
    };
    let sites = [
        site(
            &rigs,
            "Backyard",
            json!({"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0}),
        )
        .await,
        site(
            &rigs,
            "Dark site",
            json!({"latitude_degrees": 32.9, "longitude_degrees": -105.5, "elevation_meters": 2200.0}),
        )
        .await,
    ];
    for rig in 0..RIGS {
        let slug = format!("rig{rig}");
        let path = rigs._dir.path().join(format!("{slug}.sqlite"));
        rig_database(&path, rig);
        let context = crate::server::database_context::DatabaseContext::new(
            slug.clone(),
            format!("Rig {rig}"),
            path.to_string_lossy().into(),
            vec![rigs._dir.path().to_string_lossy().into()],
            None,
            None,
            None,
            rigs._dir.path().join("cache"),
        )
        .unwrap();
        rigs.state
            .databases
            .write()
            .unwrap()
            .insert(slug.clone(), Arc::new(context));
        let catalog = Uuid::parse_str(&guid(6, rig)).unwrap();
        let (status, reviewed) = call(
            &rigs.app,
            "POST",
            &format!("/catalogs/{slug}/rig/preview"),
            json!({"catalog_id": catalog}),
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{reviewed}");
        let (status, applied) = call(&rigs.app, "POST", &format!("/catalogs/{slug}/rig/apply"),
            json!({"plan": {"catalog_id": catalog}, "preview_digest": reviewed["data"]["preview_digest"]}), None).await;
        assert_eq!(status, StatusCode::OK, "{applied}");
        let id =
            Uuid::parse_str(applied["data"]["binding"]["rig"]["id"].as_str().unwrap()).unwrap();
        let (_, view) = call(
            &rigs.app,
            "GET",
            &format!("/catalogs/{slug}/rig/profile"),
            Value::Null,
            None,
        )
        .await;
        let pause = if rig % 2 == 0 { 600_000 } else { 0 };
        let edit = json!({
            "expected_revision": view["data"]["profile"]["revision"],
            "optics": null,
            "site": null,
            "horizon": null,
            "sky_quality": null,
            "limits": {"value": {"minimum_altitude_degrees": 20.0, "maximum_altitude_degrees": 88.0, "meridian_exclusion": {"before_ms": pause, "after_ms": pause}}, "source": {"kind":"manual"}},
        });
        let (status, saved) = call(
            &rigs.app,
            "PUT",
            &format!("/catalogs/{slug}/rig/profile"),
            edit,
            None,
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{saved}");
        let mut store = rigs.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut settings = store.observing_settings(Scope::Rig, id).unwrap();
        settings.site_id = Some(sites[rig / 2]);
        store.save_observing_settings(&settings).unwrap();
    }
    rigs
}

/// A request's body, as the pages send it, at fixed instants so it is
/// computed afresh and plans the first night from its start.
fn body(nights: u32, schedule_nights: u32) -> Value {
    json!({
        "nights": nights,
        "schedule_nights": schedule_nights,
        "start_ms": START_MS,
        "plan_from_ms": START_MS,
    })
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "timing harness; run by hand with --ignored --nocapture"]
async fn night_plans_timing() {
    if std::env::var_os("NIGHT_PLANS_PHASES").is_some() {
        let _ = tracing_subscriber::fmt()
            .with_env_filter("psf_guard::server::director::night_plans=debug")
            .with_writer(std::io::stderr)
            .try_init();
    }
    let rigs = rigs().await;
    let mut shapes: Vec<(&str, Value)> = vec![
        ("Live: 1 night, 1 scheduled", body(1, 1)),
        ("Nights: 7 nights, 7 scheduled", body(7, 7)),
        ("Nights: 14 nights, 14 scheduled", body(14, 14)),
        ("Summary only: 14 nights", body(14, 0)),
        ("Nights: 31 nights, 31 scheduled", body(31, 31)),
    ];
    // Once to warm the caches a process keeps either way.
    let (status, warm) = post(&rigs.app, "/night-plans", &body(1, 1)).await;
    assert_eq!(status, StatusCode::OK);
    let warm: Value = serde_json::from_slice(&warm).unwrap();
    let planned = warm["data"]["rigs"].as_array().unwrap();
    assert_eq!(planned.len(), RIGS, "{}", warm["data"]["warnings"]);
    for rig in planned {
        assert!(rig["warnings"].as_array().unwrap().is_empty(), "{rig}");
    }
    // One rig's first night opened from the grid, as Night details asks
    // for it: with every night of the grid, or only up to the one opened.
    let rig = &planned[0];
    for (label, nights) in [
        ("Detail: night 1, grid of 14", 14),
        ("Detail: night 1, 1 night", 1),
    ] {
        let mut detail = body(nights, 1);
        detail["rigs"] = json!([rig["rig"]["id"]]);
        detail["explain"] = rig["nights"][0]["date"].clone();
        shapes.push((label, detail));
    }
    const RUNS: usize = 5;
    let dump = std::env::var_os("NIGHT_PLANS_DUMP").map(std::path::PathBuf::from);
    let skies = &rigs.state.director.as_ref().unwrap().skies;
    for (label, body) in shapes {
        let mut bytes = 0;
        let mut medians = Vec::new();
        for kept in [false, true] {
            let mut times = Vec::new();
            for _ in 0..RUNS {
                if !kept {
                    skies.clear();
                }
                let started = std::time::Instant::now();
                let (status, answer) = post(&rigs.app, "/night-plans", &body).await;
                times.push(started.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{}",
                    String::from_utf8_lossy(&answer)
                );
                bytes = answer.len();
                if let Some(dump) = &dump {
                    let name: String = label
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                        .collect();
                    std::fs::write(dump.join(format!("{name}.json")), &answer).unwrap();
                }
            }
            times.sort_by(f64::total_cmp);
            medians.push(times[RUNS / 2]);
        }
        eprintln!(
            "{label:34} first {:8.1} ms  again {:8.1} ms  {:6} KiB",
            medians[0],
            medians[1],
            bytes / 1024
        );
    }
}

/// A POST and its whole body, unparsed: night plans run past the 100 kB
/// that `call` reads.
pub(super) async fn post(app: &Router, path: &str, body: &Value) -> (StatusCode, Vec<u8>) {
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/director/v1{path}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, bytes.to_vec())
}
