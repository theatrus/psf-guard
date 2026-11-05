use super::pairing::{client_call, credential, fixture};
use super::*;

fn sample(profile: Uuid) -> Value {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    json!({"sample_id":Uuid::new_v4(),"profile_id":profile,"reported_at_ms":now,
        "source_name":"NINA weather","source_key":"a".repeat(64),"connected":true,
        "values":{"cloud_percent":15.0,"temperature_celsius":7.5,"wind_speed_m_s":3.25}})
}

#[tokio::test]
async fn paired_weather_is_scoped_and_available_without_an_active_acquisition() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let path = format!("/rigs/{rig}/weather");
    let report = sample(profile);
    let (status, body) =
        client_call(&app, "POST", &path, report.clone(), token, Some(profile)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["sample_id"], report["sample_id"]);
    assert_eq!(body["data"]["stored"], true);
    assert_eq!(body["data"]["weather"]["reported_cloud"]["state"], "clear");
    assert_eq!(
        body["data"]["weather"]["forecast_conditions"]["state"],
        "unknown"
    );
    assert_eq!(
        body["data"]["weather"]["reported"]["sample"]["values"]["temperature_celsius"],
        7.5
    );
    let (_, repeated) = client_call(&app, "POST", &path, report, token, Some(profile)).await;
    assert_eq!(repeated["data"]["stored"], false);
    let (status, read) = client_call(&app, "GET", &path, Value::Null, token, Some(profile)).await;
    assert_eq!(status, StatusCode::OK, "{read}");
    assert_eq!(read["data"]["rig_id"], rig.to_string());
    assert_eq!(read["data"]["reported_cloud"]["state"], "clear");
    assert!(state
        .director
        .as_ref()
        .unwrap()
        .clone()
        .query(move |store| store.weather_observation(rig))
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn weather_credentials_cannot_cross_rigs_profiles_or_operator_routes() {
    let (_dir, _state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    for (method, path, body, header, expected) in [
        (
            "GET",
            format!("/rigs/{}/weather", Uuid::new_v4()),
            Value::Null,
            Some(profile),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "GET",
            format!("/rigs/{rig}/weather"),
            Value::Null,
            Some(Uuid::new_v4()),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "GET",
            format!("/rigs/{rig}/weather/history"),
            Value::Null,
            Some(profile),
            StatusCode::UNAUTHORIZED,
        ),
        (
            "POST",
            format!("/rigs/{rig}/weather"),
            sample(Uuid::new_v4()),
            Some(profile),
            StatusCode::FORBIDDEN,
        ),
    ] {
        let (status, response) = client_call(&app, method, &path, body, token, header).await;
        assert_eq!(status, expected, "{response}");
    }
}

#[tokio::test]
async fn invalid_weather_does_not_become_zero_cloud_or_leak_into_storage() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let mut bad = sample(profile);
    bad["values"]["cloud_percent"] = json!(101);
    let (status, body) = client_call(
        &app,
        "POST",
        &format!("/rigs/{rig}/weather"),
        bad,
        token,
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(state
        .director
        .as_ref()
        .unwrap()
        .clone()
        .query(move |store| store.weather_observation(rig))
        .await
        .unwrap()
        .is_none());
}
