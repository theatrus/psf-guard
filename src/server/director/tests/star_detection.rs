use super::pairing::{client_call, credential, fixture};
use super::*;

fn snapshot(profile: Uuid, crop: f64) -> Value {
    json!({
        "protocol_version": 1,
        "profile_id": profile,
        "profile_name": "C925",
        "captured_at": "2026-11-02T21:00:00Z",
        "nina_version": "3.2.0.9001",
        "hocus_focus_version": "4.0.0.21",
        "settings": {
            "star_detector": "hocus_focus",
            "autofocus_inner_crop_ratio": crop,
            "autofocus_brightest_stars": 30,
            "hocus_focus": {"UseAdvanced": true, "StructureLayers": 6},
        },
    })
}

#[tokio::test]
async fn a_paired_rig_records_its_profiles_star_detection_once() {
    let (dir, state, app, instance, catalog, rig) = fixture().await;
    state.set_registry_path(Some(dir.path().join("registry.json")));
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let path = format!("/rigs/{rig}/star-detection-settings");

    let (status, body) = client_call(
        &app,
        "POST",
        &path,
        snapshot(profile, 1.0),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["success"], true);
    assert_eq!(body["status"], "ready");
    assert_eq!(body["error"], Value::Null);
    assert_eq!(body["data"]["stored"], true);
    // The same snapshot again stores nothing; a changed one does.
    let (_, again) = client_call(
        &app,
        "POST",
        &path,
        snapshot(profile, 1.0),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(again["data"]["stored"], false);
    let (_, changed) = client_call(
        &app,
        "POST",
        &path,
        snapshot(profile, 0.7),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(changed["data"]["stored"], true);

    let settings = crate::star_detection::current();
    let records = &settings.profiles[&profile.to_string()].records;
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|record| record.source
        == crate::star_detection::RecordSource::DirectorPlugin
        && record.catalog_id.as_deref() == Some(catalog.to_string().as_str())));
    drop(settings);

    // Another profile than the paired one, or a catalog in the body, is refused.
    let other = Uuid::new_v4();
    let (status, _) = client_call(
        &app,
        "POST",
        &path,
        snapshot(other, 1.0),
        token,
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let mut with_catalog = snapshot(profile, 0.5);
    with_catalog["catalog_id"] = json!("db-1");
    let (status, _) = client_call(&app, "POST", &path, with_catalog, token, Some(profile)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    // Another rig's path is refused before the handler.
    let (status, _) = client_call(
        &app,
        "POST",
        &format!("/rigs/{}/star-detection-settings", Uuid::new_v4()),
        snapshot(profile, 0.5),
        token,
        Some(profile),
    )
    .await;
    assert!(status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN);
}
