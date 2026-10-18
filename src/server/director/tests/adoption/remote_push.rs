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
        ts_sync: None,
        id: "observatory".into(),
        name: "Observatory".into(),
        db_path: path.to_string_lossy().into_owned(),
        image_dirs: vec![dir.to_string_lossy().into_owned()],
        reject_archive: None,
        remote_image_upload: Some(access),
        export_dir: None,
        process_dir: None,
        autoimport: None,
        scan_rules: None,
        analyze_new_frames: false,
        pair_calibrated_copies: true,
        scan_calibrated_copies: false,
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

/// A registered peer named `obs` at `url`, set as the fixture rig's remote
/// site. Returns the registry, to point the peer elsewhere later.
async fn remote_rig(
    a: &super::activation::Activated,
    url: String,
) -> (DbRegistry, std::path::PathBuf) {
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
    set_site(a, json!("obs")).await;
    (registry, registry_path)
}

async fn set_site(a: &super::activation::Activated, peer: Value) {
    let (_, view) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/profile",
        Value::Null,
        None,
    )
    .await;
    let revision = view["data"]["profile"]["revision"].clone();
    let (status, saved) = call(
        &a.f.app,
        "PUT",
        "/catalogs/rig/rig/profile",
        profile_edit(&revision, peer),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
}

/// A loopback address nothing listens on.
fn closed_url() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

async fn activate(a: &super::activation::Activated) -> Value {
    let (status, preview) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/preview", a.project),
        json!({}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        &a.f.app,
        "POST",
        &format!("/projects/{}/activation/apply", a.project),
        json!({"preview_digest": preview["data"]["preview_digest"]}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    applied
}

async fn last(a: &super::activation::Activated) -> Value {
    let (status, last) = call(
        &a.f.app,
        "GET",
        &format!("/projects/{}/activation", a.project),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{last}");
    last["data"]["activation"].clone()
}

fn projects(db: &Connection) -> i64 {
    db.query_row("SELECT count(*) FROM project", [], |row| row.get(0))
        .unwrap()
}

#[tokio::test]
async fn a_site_offline_at_apply_is_not_sent_until_it_answers_then_gets_it_once() {
    let a = activated().await;
    let (mut registry, registry_path) = remote_rig(&a, closed_url()).await;

    // Apply writes here; the site cannot be reached, and the record says so.
    let applied = activate(&a).await;
    let push = &applied["data"]["rigs"][0]["push"];
    assert_eq!(push["applied"], false, "{applied}");
    assert!(push["error"].is_string(), "{applied}");
    let record = last(&a).await;
    let pushes = record["pushes"].as_array().unwrap();
    assert_eq!(pushes.len(), 1, "{record}");
    assert_eq!(pushes[0]["rig_id"], a.rig.to_string());
    assert_eq!(pushes[0]["peer_id"], "obs");
    assert_eq!(pushes[0]["peer_name"], "Observatory");
    assert_eq!(pushes[0]["applied"], false);
    assert!(pushes[0]["error"].is_string(), "{record}");
    assert!(pushes[0]["attempted_at_ms"].as_u64().unwrap() > 0);

    // The site comes back; a check reaches it, and the activation goes in
    // the background.
    let (url, peer_db) = peer_server(a.f._dir.path()).await;
    registry.peers[0].base_url = url;
    registry.peers[0].name = "observatory-pi".into();
    registry.save(&registry_path).unwrap();
    assert_eq!(projects(&peer_db), 0);
    let checked = crate::server::peers::check_peer(
        axum::extract::State(a.f.state.clone()),
        axum::extract::Path("obs".to_string()),
    )
    .await
    .ok()
    .unwrap();
    assert!(checked.0.data.unwrap().reachable);
    let service = a.f.state.director.clone().unwrap();
    let sent = tokio::time::timeout(std::time::Duration::from_secs(30), async {
        loop {
            let record = last(&a).await;
            if record["pushes"][0]["applied"] == true
                && service.resending.lock().unwrap().is_empty()
            {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("the resend lands");
    assert_eq!(sent["revision"], 1, "a resend is no new activation");
    assert_eq!(sent["pushes"][0]["error"], Value::Null);
    assert_eq!(sent["pushes"][0]["peer_name"], "observatory-pi");
    assert_eq!(projects(&peer_db), 1);
    let guid: String =
        a.db.query_row("SELECT guid FROM project", [], |r| r.get(0))
            .unwrap();
    let peer_guid: String = peer_db
        .query_row("SELECT guid FROM project", [], |r| r.get(0))
        .unwrap();
    assert_eq!(guid, peer_guid);

    // Another contact sends nothing: the site has it.
    peer_db.execute("DELETE FROM exposureplan", []).unwrap();
    let again = crate::server::director::peer_reached(&a.f.state, "obs")
        .expect("no resend runs now")
        .await
        .unwrap();
    assert!(again.is_empty(), "{again:?}");
    assert_eq!(
        peer_db
            .query_row("SELECT count(*) FROM exposureplan", [], |r| r
                .get::<_, i64>(0))
            .unwrap(),
        0,
        "an activation the site has is never sent again on its own"
    );
}

#[tokio::test]
async fn a_resend_runs_once_per_peer_and_only_where_rig_databases_may_be_written() {
    let a = activated().await;
    let _ = remote_rig(&a, closed_url()).await;
    activate(&a).await;
    let service = a.f.state.director.clone().unwrap();
    // One resend per peer at a time: a contact while one runs adds none.
    service.resending.lock().unwrap().insert("obs".into());
    assert!(crate::server::director::peer_reached(&a.f.state, "obs").is_none());
    service.resending.lock().unwrap().remove("obs");
    // A server that may not write rig databases sends nothing on its own.
    a.f.state.set_allow_database_management(false);
    assert!(crate::server::director::peer_reached(&a.f.state, "obs").is_none());
    a.f.state.set_allow_database_management(true);
    // Still unreachable: one try per contact, and the record says when.
    let before = last(&a).await["pushes"][0]["attempted_at_ms"]
        .as_u64()
        .unwrap();
    let tried = crate::server::director::peer_reached(&a.f.state, "obs")
        .unwrap()
        .await
        .unwrap();
    assert_eq!(tried, vec![("rig".to_string(), false)]);
    let record = last(&a).await;
    assert_eq!(record["pushes"][0]["applied"], false);
    assert!(record["pushes"][0]["attempted_at_ms"].as_u64().unwrap() >= before);
}

#[tokio::test]
async fn a_replaced_activation_is_never_resent() {
    let a = activated().await;
    let (mut registry, registry_path) = remote_rig(&a, closed_url()).await;
    activate(&a).await;
    assert_eq!(last(&a).await["pushes"][0]["applied"], false);

    // The rig runs here now, and a newer activation replaces the one the
    // site missed: nothing waits for the site any more.
    set_site(&a, Value::Null).await;
    let applied = activate(&a).await;
    assert_eq!(applied["data"]["rigs"][0]["push"], Value::Null);
    let record = last(&a).await;
    assert_eq!(record["revision"], 2);
    assert!(record.get("pushes").is_none(), "{record}");

    let (url, peer_db) = peer_server(a.f._dir.path()).await;
    registry.peers[0].base_url = url;
    registry.save(&registry_path).unwrap();
    let sent = crate::server::director::peer_reached(&a.f.state, "obs")
        .unwrap()
        .await
        .unwrap();
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(projects(&peer_db), 0);

    // A record kept from before still reads as not sent, but the rig no
    // longer sends to that site, so the page does not offer it.
    {
        let mut store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
        let mut record = store.activation(a.project).unwrap().unwrap();
        record.pushes = vec![psf_guard_director_meta::activation::RemotePush {
            rig_id: a.rig,
            peer_id: "obs".into(),
            peer_name: "Observatory".into(),
            applied: false,
            attempted_at_ms: Some(1),
            error: Some("connection refused".into()),
        }];
        assert!(store.record_pushes(a.project, 2, &record.pushes).unwrap());
    }
    assert!(last(&a).await.get("pushes").is_none());
    let sent = crate::server::director::peer_reached(&a.f.state, "obs")
        .unwrap()
        .await
        .unwrap();
    assert!(sent.is_empty(), "{sent:?}");
    assert_eq!(projects(&peer_db), 0);
}

#[tokio::test]
async fn a_plan_with_no_remote_rig_records_and_sends_no_push() {
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
    a.f.state.set_registry_path(Some(registry_path));
    let applied = activate(&a).await;
    assert_eq!(applied["data"]["rigs"][0]["push"], Value::Null);
    assert!(last(&a).await.get("pushes").is_none());
    let sent = crate::server::director::peer_reached(&a.f.state, "obs")
        .unwrap()
        .await
        .unwrap();
    assert!(sent.is_empty());
    assert_eq!(projects(&peer_db), 0);
}
