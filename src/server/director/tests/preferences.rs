use super::pairing::{client_call, credential, fixture};
use super::*;

#[tokio::test]
async fn preferences_are_operator_owned_revisioned_and_scoped() {
    let (_dir, _state, app, instance, catalog, rig) = fixture().await;
    let (_, defaults) = call(&app, "GET", "/preferences", Value::Null, None).await;
    assert_eq!(defaults["data"]["global_id"], instance.to_string());
    let path = format!("/preferences/rig/{rig}");
    let (_, value) = call(&app, "GET", &path, Value::Null, None).await;
    let mut settings = value["data"].clone();
    settings["enabled"] = true.into();
    settings["overrides"]["importance"] = 0.into();
    let profile = Uuid::new_v4();
    let c = credential(&app, instance, catalog, rig, profile).await;
    assert_eq!(
        client_call(
            &app,
            "PUT",
            &path,
            settings.clone(),
            c["token"].as_str().unwrap(),
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    let (status, saved) = call(&app, "PUT", &path, settings.clone(), None).await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["revision"], 1);
    assert_eq!(
        call(&app, "PUT", &path, settings, None).await.0,
        StatusCode::CONFLICT
    );
    let (_, effective) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/preferences"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(effective["data"]["enabled"], true);
    assert_eq!(effective["data"]["resolved"]["policy"]["importance"], 0);
    assert_eq!(
        effective["data"]["resolved"]["provenance"]["importance"]["scope"],
        "rig"
    );
}
