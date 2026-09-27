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
    assert_eq!(rig["custom_horizon"], true);
    assert_eq!(rig["in_plan"], true);
    let nights = rig["nights"].as_array().unwrap();
    assert_eq!(nights.len(), 3);
    assert_eq!(nights[0]["date"], "2026-09-25");
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
        .all(|s| s["targets"][0]["horizon_altitude_degrees"].is_number()));
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
    let bare = {
        let mut store = a.f.state.director.as_ref().unwrap().store.lock().unwrap();
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
