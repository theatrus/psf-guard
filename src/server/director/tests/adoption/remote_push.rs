//! A rig whose database lives on another PSF Guard: activation writes the
//! local copy, then sends the planning rows to the peer by Sync.

use super::activation::activated;
use super::*;
use crate::db_registry::{DbEntry, DbRegistry, PeerEntry, RemoteImageUploadConfig};
use crate::server::remote_sync;
use axum::routing::{get, post};

const TOKEN: &str = "observatory-key-long-enough-to-pass";

/// A second PSF Guard on a loopback port, holding the remote rig's database
/// and opened for sync with one token.
async fn peer_server(dir: &std::path::Path) -> (String, Connection) {
    let path = dir.join("peer.sqlite");
    let db = crate::ts_schema::create_fresh_db(&path).unwrap();
    let mut access = RemoteImageUploadConfig::default();
    access.set_token(TOKEN).unwrap();
    access.sync_enabled = true;
    let entry = DbEntry {
        id: "observatory".into(),
        name: "Observatory".into(),
        db_path: path.to_string_lossy().into_owned(),
        image_dirs: vec![dir.to_string_lossy().into_owned()],
        reject_archive: None,
        remote_image_upload: Some(access),
        export_dir: None,
        process_dir: None,
        autoimport: None,
    };
    let state = Arc::new(
        AppState::from_databases(
            vec![entry],
            dir.join("peer-cache").to_string_lossy().into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    state.set_allow_database_management(true);
    let router = Router::new()
        .route("/api/sync/v1/capabilities", get(remote_sync::capabilities))
        .route("/api/sync/v1/previews", post(remote_sync::create_preview))
        .route(
            "/api/sync/v1/previews/{preview_id}/apply",
            post(remote_sync::apply_preview),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    (url, db)
}

fn profile_edit(revision: &Value, peer: Value) -> Value {
    json!({
        "expected_revision": revision,
        "optics": null, "site": null, "horizon": null, "sky_quality": null,
        "limits": {"value": {"minimum_altitude_degrees": 20.0, "maximum_altitude_degrees": 90.0, "meridian_exclusion": {"before_ms": 0, "after_ms": 0}}, "source": {"kind": "manual"}},
        "peer_id": peer,
    })
}

#[tokio::test]
async fn activation_pushes_planning_rows_to_the_rig_peer_and_can_push_again() {
    let a = activated().await;
    let (url, peer_db) = peer_server(a.f._dir.path()).await;
    let registry_path = a.f._dir.path().join("registry.json");
    let mut registry = DbRegistry::default();
    registry.peers.push(PeerEntry {
        id: "obs".into(),
        name: "Observatory".into(),
        base_url: url,
        token: TOKEN.into(),
        catalog_id: None,
    });
    registry.save(&registry_path).unwrap();
    a.f.state.set_registry_path(Some(registry_path.clone()));
    let count =
        |db: &Connection, sql: &str| db.query_row(sql, [], |row| row.get::<_, i64>(0)).unwrap();

    // Only a registered peer can be named on the rig.
    let (_, view) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/profile",
        Value::Null,
        None,
    )
    .await;
    let revision = view["data"]["profile"]["revision"].clone();
    assert_eq!(view["data"]["profile"]["peer_id"], Value::Null);
    let (status, _) = call(
        &a.f.app,
        "PUT",
        "/catalogs/rig/rig/profile",
        profile_edit(&revision, json!("nobody")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, saved) = call(
        &a.f.app,
        "PUT",
        "/catalogs/rig/rig/profile",
        profile_edit(&revision, json!("obs")),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["profile"]["peer_id"], "obs");

    // Preview names the peer without touching it; Apply commits here, then pushes.
    let preview_path = format!("/projects/{}/activation/preview", a.project);
    let apply_path = format!("/projects/{}/activation/apply", a.project);
    let push_path = format!("/projects/{}/activation/push", a.project);
    let (status, preview) = call(&a.f.app, "POST", &preview_path, json!({}), None).await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let push = &preview["data"]["rigs"][0]["push"];
    assert_eq!(push["peer_name"], "Observatory");
    assert_eq!(push["applied"], false);
    assert_eq!(count(&peer_db, "SELECT count(*) FROM project"), 0);

    let (status, applied) = call(
        &a.f.app,
        "POST",
        &apply_path,
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let rig = &applied["data"]["rigs"][0];
    assert_eq!(rig["applied"], true);
    assert_eq!(rig["push"]["applied"], true, "{applied}");
    assert_eq!(rig["push"]["error"], Value::Null);
    assert!(rig["push"]["summary"].is_object());
    let local_guid: String =
        a.db.query_row("SELECT guid FROM project", [], |r| r.get(0))
            .unwrap();
    let peer_guid: String = peer_db
        .query_row(
            "SELECT guid FROM project WHERE name='Heart Nebula'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        local_guid, peer_guid,
        "the peer keeps the same project identity"
    );
    assert_eq!(
        count(
            &peer_db,
            "SELECT count(*) FROM target WHERE name IN ('IC 1805 r1c1','IC 1805 r2c1')"
        ),
        2
    );
    assert_eq!(
        count(
            &peer_db,
            "SELECT count(*) FROM exposureplan WHERE desired=72 AND exposure=300.0"
        ),
        2
    );
    assert_eq!(
        count(
            &peer_db,
            "SELECT count(*) FROM exposuretemplate WHERE name='Ha 300'"
        ),
        1
    );
    assert_eq!(count(&peer_db, "SELECT count(*) FROM acquiredimage"), 0);

    // Pushing again sends the same rows and changes nothing.
    let (status, again) = call(&a.f.app, "POST", &push_path, json!({}), None).await;
    assert_eq!(status, StatusCode::OK, "{again}");
    assert_eq!(again["data"]["activation_revision"], 1);
    assert_eq!(again["data"]["rigs"][0]["catalog_name"], "RedCat rig");
    assert_eq!(again["data"]["rigs"][0]["push"]["applied"], true);
    assert_eq!(count(&peer_db, "SELECT count(*) FROM project"), 1);
    assert_eq!(count(&peer_db, "SELECT count(*) FROM target"), 2);

    // A peer that cannot be reached: the call succeeds and the row says why.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        format!("http://{}", listener.local_addr().unwrap())
    };
    registry.peers[0].base_url = closed;
    registry.save(&registry_path).unwrap();
    let (status, failed) = call(&a.f.app, "POST", &push_path, json!({}), None).await;
    assert_eq!(status, StatusCode::OK, "{failed}");
    assert_eq!(failed["data"]["rigs"][0]["push"]["applied"], false);
    assert!(
        failed["data"]["rigs"][0]["push"]["error"].is_string(),
        "{failed}"
    );

    // A peer removed from the registry: the plan stays local and the preview says so.
    registry.peers.clear();
    registry.save(&registry_path).unwrap();
    let (_, orphaned) = call(&a.f.app, "POST", &preview_path, json!({}), None).await;
    assert_eq!(orphaned["data"]["rigs"][0]["push"], Value::Null);
    assert!(
        orphaned["data"]["rigs"][0]["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("no longer registered")),
        "{orphaned}"
    );
    let (status, gone) = call(&a.f.app, "POST", &push_path, json!({}), None).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        gone["data"]["rigs"][0]["push"]["error"]
            .as_str()
            .unwrap()
            .contains("no longer registered"),
        "{gone}"
    );

    // Nothing activated yet: nothing to push.
    let bare = {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        store.create_project(Uuid::new_v4(), "Bare").unwrap().id
    };
    assert_eq!(
        call(
            &a.f.app,
            "POST",
            &format!("/projects/{bare}/activation/push"),
            json!({}),
            None
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    let _ = a.objective;
}
