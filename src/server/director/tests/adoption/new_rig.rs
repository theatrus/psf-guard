use super::activation::{activated, Activated};
use super::*;
use crate::db_registry::{RemoteClient, RemoteImageUploadConfig};
use psf_guard_director_meta::SYNC_PLACED_PROFILE;

async fn preview_and_apply(a: &Activated) -> Value {
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

/// Pair a Sync plugin with the fixture's database.
fn pair_sync_plugin(a: &Activated) {
    let path = a.f._dir.path();
    let config = RemoteImageUploadConfig {
        sync_enabled: true,
        clients: vec![RemoteClient {
            client_uuid: "client".into(),
            name: "NINA".into(),
            token_salt: "salt".into(),
            token_sha256: "hash".into(),
            paired_at: 1,
        }],
        ..Default::default()
    };
    let context = crate::server::database_context::DatabaseContext::new(
        "rig".into(),
        "RedCat rig".into(),
        path.join("rig.sqlite").to_string_lossy().into(),
        vec![path.to_string_lossy().into()],
        Some(config),
        None,
        None,
        path.join("cache"),
    )
    .unwrap();
    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(context));
}

fn profiles(db: &Connection, table: &str) -> Vec<String> {
    db.prepare(&format!(
        "SELECT DISTINCT profileId FROM {table} ORDER BY 1"
    ))
    .unwrap()
    .query_map([], |row| row.get(0))
    .unwrap()
    .map(Result::unwrap)
    .collect()
}

#[tokio::test]
async fn a_blank_rig_activates_with_no_frames_and_no_plugin() {
    let a = activated().await;
    a.db.execute("DELETE FROM exposuretemplate", []).unwrap();

    let applied = preview_and_apply(&a).await;

    let rig = &applied["data"]["rigs"][0];
    assert_eq!(rig["applied"], true, "{applied}");
    assert_eq!(rig["profile_id"], SYNC_PLACED_PROFILE);
    assert!(
        rig["warnings"]
            .to_string()
            .contains("Pair the PSF Guard Sync plugin"),
        "{applied}"
    );
    for table in [
        "project",
        "exposuretemplate",
        "exposureplan",
        "profilepreference",
    ] {
        assert_eq!(profiles(&a.db, table), [SYNC_PLACED_PROFILE], "{table}");
    }
}

#[tokio::test]
async fn a_sync_plugin_rig_leaves_a_new_project_for_the_plugin_to_place() {
    let a = activated().await;
    pair_sync_plugin(&a);
    // Two N.I.N.A. profiles, and the one with more rows is not the open one.
    for (profile, name) in [
        ("profile-a", "Old one"),
        ("profile-a", "Old two"),
        ("profile-b", "Other"),
    ] {
        a.db.execute(
            "INSERT INTO project (profileId, name, description, state, priority, isMosaic, flatsHandling, guid,
                minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow)
             VALUES (?1, ?2, '', 1, 1, 0, 0, ?3, 30, 0, 0, 0, 0, 0)",
            rusqlite::params![profile, name, Uuid::new_v4().to_string()],
        )
        .unwrap();
    }

    let applied = preview_and_apply(&a).await;

    let rig = &applied["data"]["rigs"][0];
    assert_eq!(rig["profile_id"], SYNC_PLACED_PROFILE, "{applied}");
    assert!(
        rig["warnings"]
            .to_string()
            .contains("The Sync plugin adds this project"),
        "{applied}"
    );
    let placed: i64 =
        a.db.query_row(
            "SELECT COUNT(*) FROM project WHERE profileId=?1",
            [SYNC_PLACED_PROFILE],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(placed, 1);
    // Projects N.I.N.A. already placed stay where they are.
    let others: i64 =
        a.db.query_row(
            "SELECT COUNT(*) FROM project WHERE profileId IN ('profile-a','profile-b')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(others, 3);
    // The plan uses the rig's own template, which the plugin finds in Target
    // Scheduler by GUID, rather than a default copy under the waiting profile.
    assert_eq!(profiles(&a.db, "exposuretemplate"), ["profile-a"]);
    let templates: i64 =
        a.db.query_row("SELECT COUNT(*) FROM exposuretemplate", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(templates, 1);
}

#[tokio::test]
async fn a_project_the_plugin_placed_keeps_its_profile_on_the_next_activation() {
    let a = activated().await;
    pair_sync_plugin(&a);
    preview_and_apply(&a).await;
    // What a pull-back of the plugin's write leaves in the catalog.
    for table in ["project", "exposureplan", "exposuretemplate"] {
        a.db.execute(
            &format!("UPDATE {table} SET profileId='real' WHERE profileId=?1"),
            [SYNC_PLACED_PROFILE],
        )
        .unwrap();
    }

    let applied = preview_and_apply(&a).await;

    assert_eq!(applied["data"]["rigs"][0]["applied"], true, "{applied}");
    assert!(!profiles(&a.db, "project").contains(&SYNC_PLACED_PROFILE.to_owned()));
    assert!(!profiles(&a.db, "exposureplan").contains(&SYNC_PLACED_PROFILE.to_owned()));
}

#[tokio::test]
async fn a_database_read_directly_keeps_using_a_profile_it_has() {
    let a = activated().await;

    let applied = preview_and_apply(&a).await;

    assert_eq!(applied["data"]["rigs"][0]["profile_id"], "profile-a");
    assert_eq!(profiles(&a.db, "project"), ["profile-a"]);
}

#[tokio::test]
async fn removing_a_rig_previews_what_goes_then_deletes_it() {
    let a = activated().await;
    preview_and_apply(&a).await;
    let (status, review) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/removal",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{review}");
    assert_eq!(review["data"]["removal"]["rig"]["id"], a.rig.to_string());
    assert_eq!(review["data"]["removal"]["plans"], json!(["Heart Nebula"]));
    assert_eq!(review["data"]["removal"]["blocker"], Value::Null);

    let catalog = a.f.state.get_database("rig").unwrap();
    assert!(matches!(
        crate::server::director::rig_removal::remove(&a.f.state, catalog.clone()).await,
        Ok(true)
    ));

    let (_, after) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/removal",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(after["data"]["removal"], Value::Null);
    // A second removal has nothing to delete.
    assert!(matches!(
        crate::server::director::rig_removal::remove(&a.f.state, catalog).await,
        Ok(false)
    ));
    let store = a.f.state.director.as_ref().unwrap().writer.lock().unwrap();
    let plan = store.plan_draft(a.project).unwrap().unwrap();
    assert!(plan.contributions.is_empty());
}

#[tokio::test]
async fn waiting_projects_count_with_the_profile_the_plugin_runs_in() {
    let a = activated().await;
    pair_sync_plugin(&a);
    // A project N.I.N.A. made, then two from activation still waiting.
    a.db.execute(
        "INSERT INTO project (profileId, name, description, state, priority, isMosaic, flatsHandling, guid,
            minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow)
         VALUES ('profile-a', 'Made in N.I.N.A.', '', 1, 1, 0, 0, ?1, 30, 0, 0, 0, 0, 0)",
        [Uuid::new_v4().to_string()],
    )
    .unwrap();
    preview_and_apply(&a).await;
    a.db.execute(
        "INSERT INTO project (profileId, name, description, state, priority, isMosaic, flatsHandling, guid,
            minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow)
         VALUES (?1, 'Second waiting', '', 1, 1, 0, 0, ?2, 30, 0, 0, 0, 0, 0)",
        rusqlite::params![SYNC_PLACED_PROFILE, Uuid::new_v4().to_string()],
    )
    .unwrap();
    let path = a.f._dir.path().join("rig.sqlite");

    let read = crate::server::director::ts_snapshot::read(path.to_str().unwrap(), None)
        .unwrap()
        .unwrap();

    assert_eq!(read.snapshot.profile_id, "profile-a");
    assert_eq!(read.snapshot.projects.len(), 3);
    assert!(
        read.warnings
            .iter()
            .all(|w| !w.contains("another N.I.N.A. profile")),
        "{:?}",
        read.warnings
    );
}

#[tokio::test]
async fn the_profile_the_plugin_reports_beats_the_larger_profile() {
    let a = activated().await;
    // The reducer profile, stored in upper case, has one project; another
    // profile has more.
    let reducer = "9F1C0D2E-0000-4000-8000-00000000000A";
    let add = |profile: &str, name: &str| {
        a.db.execute(
            "INSERT INTO project (profileId, name, description, state, priority, isMosaic, flatsHandling, guid,
                minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow)
             VALUES (?1, ?2, '', 1, 1, 0, 0, ?3, 30, 0, 0, 0, 0, 0)",
            rusqlite::params![profile, name, Uuid::new_v4().to_string()],
        )
        .unwrap();
    };
    add(reducer, "With reducer");
    for name in ["One", "Two", "Three", "Four"] {
        add("profile-b", name);
    }
    let path = a.f._dir.path().join("rig.sqlite");
    let read = |reported: Option<&str>| {
        crate::server::director::ts_snapshot::read(path.to_str().unwrap(), reported)
            .unwrap()
            .unwrap()
    };

    let reported = read(Some(&reducer.to_lowercase()));
    let unknown = read(Some("00000000-0000-4000-8000-0000000000ff"));
    let unreported = read(None);

    assert_eq!(reported.snapshot.profile_id, reducer);
    assert_eq!(reported.snapshot.projects.len(), 1);
    assert!(
        reported
            .warnings
            .iter()
            .any(|w| w.contains("another N.I.N.A. profile")),
        "{:?}",
        reported.warnings
    );
    // A profile the database does not use, or none, falls back to the count.
    assert_eq!(unknown.snapshot.profile_id, "profile-b");
    assert_eq!(unreported.snapshot.profile_id, "profile-b");
}

/// A rig database opened for sync with one token: a project N.I.N.A. made
/// under `profile-a`, and a project and template left waiting by activation.
/// Returns the planning export's profile IDs, by table, and the contact
/// the database list then shows for the remote API key.
async fn exported_profiles(
    plugin_capabilities: Option<&str>,
) -> (std::collections::BTreeMap<String, Vec<String>>, Value) {
    use crate::db_registry::DbEntry;
    use crate::server::{handlers, remote_sync, rig_plugins};
    use axum::routing::{get, post};
    use tower::ServiceExt;
    const TOKEN: &str = "rig-sync-key-long-enough-to-pass";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("rig.sqlite");
    let db = crate::ts_schema::create_fresh_db(&path).unwrap();
    for profile in ["profile-a", SYNC_PLACED_PROFILE] {
        db.execute(
            "INSERT INTO project (profileId, name, description, state, priority, isMosaic, flatsHandling, guid,
                minimumtime, minimumaltitude, maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow)
             VALUES (?1, ?1, '', 1, 1, 0, 0, ?2, 30, 0, 0, 0, 0, 0)",
            rusqlite::params![profile, Uuid::new_v4().to_string()],
        )
        .unwrap();
    }
    db.execute(
        "INSERT INTO exposuretemplate (profileId, name, filtername, gain, offset, bin, readoutmode, twilightlevel, moonavoidanceenabled,
            moonavoidanceseparation, moonavoidancewidth, maximumhumidity, defaultexposure, moonrelaxscale, moonrelaxmaxaltitude,
            moonrelaxminaltitude, moondownenabled, ditherevery, minutesOffset, guid)
         VALUES (?1, 'Ha 300', 'Ha', 100, 30, 1, -1, 0, 0, 60, 7, 0, 300, 0, 5, -15, 0, -1, 0, ?2)",
        rusqlite::params![SYNC_PLACED_PROFILE, Uuid::new_v4().to_string()],
    )
    .unwrap();
    let mut access = RemoteImageUploadConfig::default();
    access.set_token(TOKEN).unwrap();
    access.sync_enabled = true;
    let entry = DbEntry {
        ts_sync: None,
        id: "rig".into(),
        name: "Rig".into(),
        db_path: path.to_string_lossy().into_owned(),
        image_dirs: vec![dir.path().to_string_lossy().into_owned()],
        reject_archive: None,
        remote_image_upload: Some(access),
        export_dir: None,
        process_dir: None,
        autoimport: None,
        scan_rules: None,
        analyze_new_frames: false,
        pair_calibrated_copies: true,
        scan_calibrated_copies: false,
        processing_overrides: None,
    };
    let state = Arc::new(
        AppState::from_databases(
            vec![entry],
            dir.path().join("cache").to_string_lossy().into_owned(),
            PregenerationConfig::default(),
        )
        .unwrap(),
    );
    let router = axum::Router::new()
        .route("/api/sync/v1/exports", post(remote_sync::create_export))
        .route("/api/databases", get(handlers::list_databases))
        .with_state(state);
    let mut request = axum::http::Request::builder()
        .method("POST")
        .uri("/api/sync/v1/exports")
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {TOKEN}"))
        .header(rig_plugins::PLUGIN_VERSION_HEADER, "0.4.0.99");
    if let Some(capabilities) = plugin_capabilities {
        request = request.header(rig_plugins::PLUGIN_CAPABILITIES_HEADER, capabilities);
    }
    let body = json!({"protocol_version": 1, "catalog_id": "rig", "operation": "push_planning"});
    let response = router
        .clone()
        .oneshot(
            request
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let export: Value = serde_json::from_slice(&bytes).unwrap();
    let listed = router
        .oneshot(
            axum::http::Request::get("/api/databases")
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let listed: Value = serde_json::from_slice(
        &axum::body::to_bytes(listed.into_body(), usize::MAX)
            .await
            .unwrap(),
    )
    .unwrap();
    let contact = listed["data"][0]["remote_image_upload"]["key_contact"].clone();
    let tables = export["data"]["bundle"]["tables"].as_object().unwrap();
    let profiles = tables
        .iter()
        .filter_map(|(name, table)| {
            let column = table["columns"]
                .as_array()?
                .iter()
                .position(|column| column["name"] == "profileId")?;
            let profiles = table["rows"]
                .as_array()?
                .iter()
                .map(|row| {
                    row["values"][column]["value"]
                        .as_str()
                        .unwrap_or("")
                        .to_owned()
                })
                .collect();
            Some((name.clone(), profiles))
        })
        .collect();
    (profiles, contact)
}

#[tokio::test]
async fn an_older_sync_plugin_gets_waiting_rows_under_the_rigs_profile() {
    let (old, old_contact) = exported_profiles(None).await;
    let (current, current_contact) = exported_profiles(Some("waiting_profile_v1")).await;

    assert_eq!(old["project"], ["profile-a", "profile-a"]);
    assert_eq!(old["exposuretemplate"], ["profile-a"]);
    assert_eq!(current["project"], ["profile-a", SYNC_PLACED_PROFILE]);
    assert_eq!(current["exposuretemplate"], [SYNC_PLACED_PROFILE]);
    // Settings shows what each plugin reported.
    assert_eq!(old_contact["version"], "0.4.0.99");
    assert_eq!(old_contact["capabilities"], json!([]));
    assert_eq!(old_contact["places_new_projects"], false);
    assert_eq!(
        current_contact["capabilities"],
        json!(["waiting_profile_v1"])
    );
    assert_eq!(current_contact["places_new_projects"], true);
}

#[tokio::test]
async fn the_real_profiles_preferences_beat_the_waiting_profiles() {
    let a = activated().await;
    // Activation on a blank rig left a waiting preference row first.
    crate::commands::import::ensure_profile_preference(&a.db, SYNC_PLACED_PROFILE).unwrap();
    crate::commands::import::ensure_profile_preference(&a.db, "profile-a").unwrap();
    a.db.execute(
        "UPDATE profilepreference SET parkonwait = 1 WHERE profileId = 'profile-a'",
        [],
    )
    .unwrap();
    let path = a.f._dir.path().join("rig.sqlite");

    let read = crate::server::director::ts_snapshot::read(path.to_str().unwrap(), None)
        .unwrap()
        .unwrap();

    assert_eq!(read.snapshot.profile_id, "profile-a");
    assert!(read.snapshot.preferences.park_on_wait);
}

#[tokio::test]
async fn removing_one_of_two_names_for_a_file_keeps_its_rig() {
    let a = activated().await;
    preview_and_apply(&a).await;
    let original = a.f.state.get_database("rig").unwrap();
    let path = a.f._dir.path();
    let twin = crate::server::database_context::DatabaseContext::new(
        "twin".into(),
        "Same file".into(),
        original.database_path.clone(),
        vec![path.to_string_lossy().into()],
        None,
        None,
        None,
        path.join("twin-cache"),
    )
    .unwrap();
    a.f.state
        .databases
        .write()
        .unwrap()
        .insert("twin".into(), Arc::new(twin));
    let twin = a.f.state.get_database("twin").unwrap();

    let (_, review) = call(
        &a.f.app,
        "GET",
        "/catalogs/twin/rig/removal",
        Value::Null,
        None,
    )
    .await;
    let removed = crate::server::director::rig_removal::remove(&a.f.state, twin).await;

    assert_eq!(review["data"]["removal"], Value::Null, "{review}");
    assert!(matches!(removed, Ok(false)));
    // The route drops the twin from the server before deleting its rig.
    a.f.state.databases.write().unwrap().remove("twin");
    let (_, kept) = call(
        &a.f.app,
        "GET",
        "/catalogs/rig/rig/removal",
        Value::Null,
        None,
    )
    .await;
    assert_eq!(
        kept["data"]["removal"]["rig"]["id"],
        a.rig.to_string(),
        "{kept}"
    );
}

#[tokio::test]
async fn activation_warns_when_a_template_filter_is_not_on_the_typed_list() {
    let a = activated().await;
    let preview_warnings = |a: &Activated| {
        let app = a.f.app.clone();
        let project = a.project;
        async move {
            let (status, preview) = call(
                &app,
                "POST",
                &format!("/projects/{project}/activation/preview"),
                json!({}),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{preview}");
            preview["data"]["rigs"][0]["warnings"].to_string()
        }
    };
    let type_filters = |revision: u64, names: Value| {
        let app = a.f.app.clone();
        async move {
            let (status, saved) = call(
                &app,
                "PUT",
                "/catalogs/rig/rig/profile",
                json!({"expected_revision": revision, "optics": null, "site": null, "horizon": null,
                    "sky_quality": null, "limits": {"value": {"minimum_altitude_degrees": 0.0,
                    "maximum_altitude_degrees": 90.0, "meridian_exclusion": {"before_ms": 0, "after_ms": 0}},
                    "source": {"kind": "manual"}},
                    "filters": {"value": names, "source": {"kind": "manual"}}}),
                None,
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{saved}");
            saved["data"]["profile"]["revision"].as_u64().unwrap()
        }
    };
    // No list, no check.
    assert!(!preview_warnings(&a).await.contains("filter list"));

    let revision = type_filters(0, json!(["L", "R", "G", "B"])).await;
    let warnings = preview_warnings(&a).await;
    assert!(
        warnings.contains("Filter Ha: not on this rig's filter list (L, R, G, B)."),
        "{warnings}"
    );

    type_filters(revision, json!(["L", "ha"])).await;
    assert!(!preview_warnings(&a).await.contains("filter list"));
}
