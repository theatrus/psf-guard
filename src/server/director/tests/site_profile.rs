use super::pairing::fixture;
use super::*;

#[tokio::test]
async fn a_site_keeps_a_location_and_a_horizon_read_from_an_hrz_file() {
    let (_dir, _state, app, _instance, _catalog, _rig) = fixture().await;
    let site = Uuid::new_v4();
    let (status, _) = call(
        &app,
        "POST",
        "/sites",
        json!({"id": site, "name": "Backyard"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let path = format!("/sites/{site}/profile");
    let (status, empty) = call(&app, "GET", &path, Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{empty}");
    assert_eq!(empty["data"]["site"]["name"], "Backyard");
    assert_eq!(empty["data"]["profile"]["revision"], 0);
    assert_eq!(empty["data"]["profile"]["horizon"], Value::Null);
    assert_eq!(
        call(
            &app,
            "GET",
            &format!("/sites/{}/profile", Uuid::new_v4()),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );

    // A file without endpoints comes back closed at 0 and 360.
    let (status, parsed) = call(
        &app,
        "POST",
        "/horizons/parse",
        json!({"text": "# trees\n90 20\n270 10\n"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{parsed}");
    let horizon = parsed["data"].clone();
    assert_eq!(horizon["mode"], "custom");
    assert_eq!(horizon["points"].as_array().unwrap().len(), 4);
    assert_eq!(horizon["points"][0]["azimuth_degrees"], 0.0);
    assert_eq!(horizon["points"][3]["azimuth_degrees"], 360.0);
    let (status, refused) = call(
        &app,
        "POST",
        "/horizons/parse",
        json!({"text": "0 10\nnorth 5\n"}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(
        refused["error"].as_str().unwrap().contains("line 2"),
        "{refused}"
    );

    let location =
        json!({"latitude_degrees": 34.2, "longitude_degrees": -118.3, "elevation_meters": 400.0});
    let edit = json!({
        "expected_revision": 0,
        "location": {"value": location, "source": {"kind": "manual"}},
        "horizon": {"value": horizon, "source": {"kind": "manual"}},
    });
    let (status, saved) = call(&app, "PUT", &path, edit.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["profile"]["revision"], 1);
    assert_eq!(saved["data"]["profile"]["horizon"]["value"], horizon);
    assert_eq!(
        call(&app, "PUT", &path, edit.clone(), None).await.0,
        StatusCode::CONFLICT
    );
    let mut plugin = edit.clone();
    plugin["expected_revision"] = 1.into();
    plugin["horizon"]["source"] = json!({"kind": "plugin"});
    assert_eq!(
        call(&app, "PUT", &path, plugin, None).await.0,
        StatusCode::BAD_REQUEST
    );
    let mut cleared = edit;
    cleared["expected_revision"] = 1.into();
    cleared["horizon"] = Value::Null;
    let (status, saved) = call(&app, "PUT", &path, cleared, None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["profile"]["revision"], 2);
    assert_eq!(saved["data"]["profile"]["horizon"], Value::Null);
    assert_eq!(saved["data"]["profile"]["location"]["value"], location);
}
