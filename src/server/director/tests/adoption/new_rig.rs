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

    let read = crate::server::director::ts_snapshot::read(path.to_str().unwrap())
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
