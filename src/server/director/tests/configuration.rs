use super::*;
use psf_guard_director_core::{
    program::Program,
    visibility::{Horizon, HorizonPoint, Site},
    windows::MeridianExclusion,
    MAX_REQUEST_BYTES,
};
use psf_guard_director_meta::configuration::{RigSetup, SiteSnapshot};

fn snapshots(site: Uuid, rig: Uuid) -> (SiteSnapshot, RigSetup) {
    let snapshot = SiteSnapshot {
        id: Uuid::new_v4(),
        site_id: site,
        location: Site {
            latitude_degrees: 35.0,
            longitude_degrees: -120.0,
            elevation_meters: 1000.0,
        },
        horizon: Horizon::Custom {
            points: (0..=200)
                .map(|n| HorizonPoint {
                    azimuth_degrees: f64::from(n) * 1.8,
                    altitude_degrees: if n == 200 { 20.0 } else { 10.0 },
                })
                .collect(),
        },
    };
    let mut program: Program = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/director-core/tests/fixtures/execution-program.json"
    )))
    .unwrap();
    program.configuration.rig_id = rig.to_string();
    program.configuration.id = "nina-equipment-fingerprint".into();
    let setup = RigSetup {
        id: Uuid::new_v4(),
        configuration: program.configuration,
        site_snapshot_id: snapshot.id,
        minimum_altitude_degrees: 20.0,
        maximum_altitude_degrees: 85.0,
        meridian_exclusion: MeridianExclusion {
            before_ms: 3_600_000,
            after_ms: 0,
        },
    };
    (snapshot, setup)
}

async fn create_parent(app: &Router, path: &str, id: Uuid) {
    assert_eq!(
        call(app, "POST", path, json!({"id":id,"name":"Same name"}), None)
            .await
            .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn identity_routes_keep_project_site_and_rig_namespaces_and_revisions_separate() {
    let dir = TempDir::new().unwrap();
    let app = router(Arc::new(state(&dir, true)));
    let id = Uuid::new_v4();
    for path in ["/projects", "/sites", "/rigs"] {
        create_parent(&app, path, id).await;
        let (status, page) = call(&app, "GET", &format!("{path}?limit=1"), Value::Null, None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(page["data"]["items"][0]["id"], id.to_string());
    }
    for path in ["/sites", "/rigs"] {
        let body = json!({"expected_revision":1,"name":path});
        assert_eq!(
            call(&app, "PATCH", &format!("{path}/{id}"), body.clone(), None)
                .await
                .1["data"]["revision"],
            2
        );
        assert_eq!(
            call(&app, "PATCH", &format!("{path}/{id}"), body, None)
                .await
                .0,
            StatusCode::CONFLICT
        );
        assert_eq!(
            call(&app, "GET", &format!("{path}/{id}"), Value::Null, None)
                .await
                .1["data"]["name"],
            path
        );
    }
    assert_eq!(
        call(&app, "GET", &format!("/projects/{id}"), Value::Null, None)
            .await
            .1["data"]["name"],
        "Same name"
    );
}

#[tokio::test]
async fn full_horizon_and_native_configuration_roundtrip_without_four_kib_truncation() {
    let dir = TempDir::new().unwrap();
    let app = router(Arc::new(state(&dir, true)));
    let site = Uuid::new_v4();
    let rig = Uuid::new_v4();
    create_parent(&app, "/sites", site).await;
    create_parent(&app, "/rigs", rig).await;
    let (snapshot, setup) = snapshots(site, rig);
    let body = serde_json::to_value(&snapshot).unwrap();
    assert!(body.to_string().len() > 4096);
    let site_path = format!("/sites/{site}/snapshots");
    let rig_path = format!("/rigs/{rig}/setups");
    assert_eq!(
        call(
            &app,
            "POST",
            &rig_path,
            serde_json::to_value(&setup).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    for _ in 0..2 {
        let (status, result) = call(&app, "POST", &site_path, body.clone(), None).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(result["data"], body);
        assert_eq!(
            call(
                &app,
                "POST",
                &rig_path,
                serde_json::to_value(&setup).unwrap(),
                None
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    let mut changed = body.clone();
    changed["location"]["elevation_meters"] = 2000.into();
    assert_eq!(
        call(&app, "POST", &site_path, changed, None).await.0,
        StatusCode::CONFLICT
    );
    let mut changed = serde_json::to_value(&setup).unwrap();
    changed["minimum_altitude_degrees"] = 25.into();
    assert_eq!(
        call(&app, "POST", &rig_path, changed, None).await.0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "GET", &site_path, Value::Null, None).await.1["data"]["ids"],
        json!([snapshot.id])
    );
    assert_eq!(
        call(&app, "GET", &rig_path, Value::Null, None).await.1["data"]["ids"],
        json!([setup.id])
    );
    drop(app);
    let reopened = router(Arc::new(state(&dir, true)));
    assert_eq!(
        call(
            &reopened,
            "GET",
            &format!("{site_path}/{}", snapshot.id),
            Value::Null,
            None
        )
        .await
        .1["data"],
        body
    );
    assert_eq!(
        call(
            &reopened,
            "GET",
            &format!("{rig_path}/{}", setup.id),
            Value::Null,
            None
        )
        .await
        .1["data"],
        serde_json::to_value(setup).unwrap()
    );
    assert_eq!(
        call(&reopened, "GET", "/status", Value::Null, None).await.1["data"]
            ["acquisition_available"],
        false
    );
}

#[tokio::test]
async fn url_scope_unknown_parents_and_local_paths_cannot_change_snapshot_ownership() {
    let dir = TempDir::new().unwrap();
    let app = router(Arc::new(state(&dir, true)));
    let site = Uuid::new_v4();
    let rig = Uuid::new_v4();
    let other = Uuid::new_v4();
    let (snapshot, setup) = snapshots(site, rig);
    for path in [
        format!("/sites/{site}/snapshots"),
        format!("/rigs/{rig}/setups"),
    ] {
        assert_eq!(
            call(&app, "GET", &path, Value::Null, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/sites/{other}/snapshots"),
            serde_json::to_value(&snapshot).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{other}/setups"),
            serde_json::to_value(&setup).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    create_parent(&app, "/sites", site).await;
    create_parent(&app, "/rigs", rig).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/sites/{site}/snapshots"),
            serde_json::to_value(&snapshot).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{rig}/setups"),
            serde_json::to_value(&setup).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    for path in [
        format!("/sites/{other}/snapshots/{}", snapshot.id),
        format!("/rigs/{other}/setups/{}", setup.id),
    ] {
        assert_eq!(
            call(&app, "GET", &path, Value::Null, None).await.0,
            StatusCode::NOT_FOUND
        );
    }
    let mut body = serde_json::to_value(snapshot).unwrap();
    body["horizon_file_path"] = "C:/not-server-data.hrz".into();
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/sites/{site}/snapshots"),
            body,
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
}

#[tokio::test]
async fn configuration_routes_bound_input_sizes_and_keep_management_gate() {
    let dir = TempDir::new().unwrap();
    let app_state = Arc::new(state(&dir, true));
    let app = router(app_state.clone());
    let site = Uuid::new_v4();
    let rig = Uuid::new_v4();
    let (snapshot, setup) = snapshots(site, rig);
    for path in ["/sites", "/rigs"] {
        assert_eq!(
            call(
                &app,
                "POST",
                path,
                json!({"id":site,"name":"x".repeat(5000)}),
                None
            )
            .await
            .0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
        assert_eq!(
            call(&app, "GET", &format!("{path}?limit=257"), Value::Null, None)
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    for (path, mut body) in [
        (
            format!("/sites/{site}/snapshots"),
            serde_json::to_value(&snapshot).unwrap(),
        ),
        (
            format!("/rigs/{rig}/setups"),
            serde_json::to_value(&setup).unwrap(),
        ),
    ] {
        body["padding"] = "x".repeat(MAX_REQUEST_BYTES).into();
        assert_eq!(
            call(&app, "POST", &path, body, None).await.0,
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }
    app_state.set_allow_database_management(false);
    for path in ["/sites", "/rigs"] {
        assert_eq!(
            call(&app, "GET", path, Value::Null, None).await.0,
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/sites/{site}/snapshots"),
            serde_json::to_value(snapshot).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{rig}/setups"),
            serde_json::to_value(setup).unwrap(),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn ordinary_authentication_and_read_only_role_protect_every_configuration_route() {
    let dir = TempDir::new().unwrap();
    let app_state = Arc::new(state(&dir, true));
    let mut registry = AuthRegistry::default();
    registry
        .add(
            AuthUserRecord::new("operator", AccessRole::ReadWrite, "test-password-not-real")
                .unwrap(),
            false,
        )
        .unwrap();
    let (reader, record) = AuthTokenRecord::mint("operator", "reader", true, None).unwrap();
    registry.tokens.push(record);
    app_state.set_server_auth(auth::ServerAuth::from_sources(None, &registry, 3000).unwrap());
    let app = router(app_state);
    let id = Uuid::new_v4();
    for path in [
        "/sites".into(),
        "/rigs".into(),
        format!("/sites/{id}/snapshots"),
        format!("/rigs/{id}/setups"),
    ] {
        for token in [None, Some("database-sync-key")] {
            assert_eq!(
                call(&app, "GET", &path, Value::Null, token).await.0,
                StatusCode::UNAUTHORIZED
            );
        }
        assert_eq!(
            call(&app, "POST", &path, json!({}), Some(&reader)).await.0,
            StatusCode::FORBIDDEN
        );
    }
    for path in [format!("/sites/{id}"), format!("/rigs/{id}")] {
        assert_eq!(
            call(&app, "PATCH", &path, json!({}), Some(&reader)).await.0,
            StatusCode::FORBIDDEN
        );
    }
    for path in ["/sites", "/rigs"] {
        assert_eq!(
            call(&app, "GET", path, Value::Null, Some(&reader)).await.0,
            StatusCode::OK
        );
    }
}
