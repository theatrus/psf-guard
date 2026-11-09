//! A paired Director plugin uploads the frames it captures through the Sync
//! upload's receive path, gated by the database's Image uploads setting.
use super::super::pairing::{client_call, credential};
use super::activation::activated;
use super::*;
use crate::server::database_context::DatabaseContext;
use axum::body::to_bytes;

/// POST an image as the plugin would, with its pairing headers.
async fn upload(
    app: &Router,
    rig: Uuid,
    token: &str,
    profile: Uuid,
    checksum: &str,
) -> (StatusCode, Value) {
    let boundary = "psf-guard-director-test";
    let mut body = format!(
        "--{boundary}\r\nContent-Disposition: form-data; name=\"image\"; filename=\"M31_L_300s_0001.fits\"\r\nContent-Type: application/fits\r\n\r\n"
    )
    .into_bytes();
    body.extend_from_slice(&[b' '; 2880]);
    body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/director/v1/rigs/{rig}/images"))
        .header(
            "content-type",
            format!("multipart/form-data; boundary={boundary}"),
        )
        .header("authorization", format!("Bearer {token}"))
        .header("x-psf-director-profile", profile.to_string())
        .header("x-content-sha256", checksum)
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn a_paired_rig_uploads_through_the_databases_image_upload_setting() {
    let a = activated().await;
    let instance = a.f.state.director.as_ref().unwrap().instance_id;
    let profile = Uuid::new_v4();
    let paired = credential(&a.f.app, instance, a.rig, a.rig, profile).await;
    let token = paired["token"].as_str().unwrap().to_owned();
    let state_path = format!("/rigs/{}/uploads", a.rig);

    // Off by default, with the reason to show in the plugin.
    let (status, body) = client_call(
        &a.f.app,
        "GET",
        &state_path,
        Value::Null,
        &token,
        Some(profile),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["enabled"], false);
    assert!(body["data"]["reason"]
        .as_str()
        .unwrap()
        .contains("Image uploads are off"));
    let (status, body) = upload(&a.f.app, a.rig, &token, profile, &"0".repeat(64)).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{body}");
    assert!(
        body["error"]
            .as_str()
            .unwrap_or_default()
            .contains("disabled"),
        "{body}"
    );
    let refused = a.f.state.remote_audit.problems("rig");
    assert_eq!(
        refused[0].action,
        crate::server::remote_audit::AuditAction::DirectorImageUpload
    );
    // Not the Sync plugin's action.
    assert!(a.f.state.remote_audit.latest("rig").is_none());

    // Turn uploads on for the database.
    // The rig's own database, which its pairing names.
    let current = a.f.state.get_database("rig").unwrap();
    let images = a.f._dir.path().join("received");
    std::fs::create_dir_all(&images).unwrap();
    let config = crate::db_registry::RemoteImageUploadConfig {
        enabled: true,
        image_dir: images.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let enabled = DatabaseContext::new(
        "rig".into(),
        current.name.clone(),
        current.database_path.clone(),
        // The receive folder is one of the database's image folders.
        vec![images.to_string_lossy().into_owned()],
        Some(config),
        None,
        None,
        a.f._dir.path().join("cache"),
    )
    .unwrap();
    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(enabled));
    let (_, body) = client_call(
        &a.f.app,
        "GET",
        &state_path,
        Value::Null,
        &token,
        Some(profile),
    )
    .await;
    assert_eq!(body["data"]["enabled"], true, "{body}");
    assert!(body["data"].get("reason").is_none());

    // The shared receiver checks the file: a wrong checksum is refused.
    let (status, body) = upload(&a.f.app, a.rig, &token, profile, &"0".repeat(64)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{body}");
    assert!(
        body["error"].as_str().unwrap().contains("SHA-256"),
        "{body}"
    );

    // Another rig's path, or no pairing, gets nothing.
    let (status, _) = client_call(
        &a.f.app,
        "GET",
        &format!("/rigs/{}/uploads", Uuid::new_v4()),
        Value::Null,
        &token,
        Some(profile),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "{status}"
    );
    let (status, _) = client_call(
        &a.f.app,
        "GET",
        &state_path,
        Value::Null,
        "psfdrc_not-a-token",
        Some(profile),
    )
    .await;
    assert!(
        status == StatusCode::FORBIDDEN || status == StatusCode::UNAUTHORIZED,
        "{status}"
    );
}

#[test]
fn a_busy_import_answers_503_so_both_plugins_retry() {
    use axum::response::IntoResponse;
    let response = crate::server::handlers::AppError::Busy(
        "another image import is already running for this database".into(),
    )
    .into_response();
    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(response.headers()["retry-after"], "5");
}
