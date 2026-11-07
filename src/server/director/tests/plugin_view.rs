use super::pairing::{client_call, credential, fixture};
use super::*;
use psf_guard_director_meta::{profile::RigMode, profile::RigProfile, CatalogIdentity};

/// A registered catalog bound to a rig, as the view reads it by slug.
fn register(dir: &TempDir, state: &Arc<AppState>) -> (Uuid, Uuid) {
    let path = dir.path().join("plugin.sqlite");
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch(
            "CREATE TABLE project(Id INTEGER PRIMARY KEY,name TEXT,guid TEXT,profileId TEXT);",
        )
        .unwrap();
    let context = super::super::super::database_context::DatabaseContext::new(
        "plugin".into(),
        "Plugin".into(),
        path.to_string_lossy().into(),
        vec![dir.path().to_string_lossy().into()],
        None,
        None,
        None,
        dir.path().join("cache"),
    )
    .unwrap();
    let database_path = context.database_path.clone();
    state
        .databases
        .write()
        .unwrap()
        .insert("plugin".into(), Arc::new(context));
    let service = state.director.as_ref().unwrap();
    let identity: CatalogIdentity =
        super::super::derived_identity(service.instance_id, &database_path);
    let rig = service
        .writer
        .lock()
        .unwrap()
        .bind_catalog_rig_after(identity, "Plugin rig", true, || Ok(()))
        .unwrap()
        .rig
        .id;
    (identity.id, rig)
}

async fn view(app: &Router) -> Value {
    let (status, body) = call(app, "GET", "/catalogs/plugin/rig/plugin", Value::Null, None).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    body["data"].clone()
}

#[tokio::test]
async fn the_view_follows_a_plugin_from_pairing_to_removal() {
    let (dir, state, app, instance, _, _) = fixture().await;
    let (catalog, rig) = register(&dir, &state);

    let empty = view(&app).await;
    assert_eq!(empty["catalog_id"], catalog.to_string());
    assert_eq!(empty["rig_id"], rig.to_string());
    assert_eq!(empty["coordinator_instance_id"], instance.to_string());
    assert_eq!(empty["clients"], json!([]));
    assert_eq!(empty["work"], "not_allowed");
    assert_eq!(empty["grant"], Value::Null);

    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let client = paired["client_id"].as_str().unwrap();
    let listed = view(&app).await;
    assert_eq!(listed["clients"][0]["client_id"], client);
    assert_eq!(listed["clients"][0]["review"], "none");
    assert!(
        !listed.to_string().contains("psfdrc_"),
        "the view never carries a credential"
    );

    let mut program: psf_guard_director_core::program::Program = serde_json::from_str(
        include_str!("../../../../crates/director-core/tests/fixtures/execution-program.json"),
    )
    .unwrap();
    program.configuration.rig_id = rig.to_string();
    let report = Uuid::new_v4();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let input = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"report_id":report,
        "observed_at_ms":now,"configuration":program.configuration,"filter_names":{"ha":"H-alpha"}});
    let path = format!("/rigs/{rig}/equipment-reports");
    let (status, body) = client_call(&app, "POST", &path, input, token, Some(profile)).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    let reported = view(&app).await;
    assert_eq!(reported["clients"][0]["review"], "accepted");
    assert_eq!(
        reported["clients"][0]["report"]["report_id"],
        report.to_string()
    );
    assert_eq!(reported["configuration"]["id"], program.configuration.id);
    assert_eq!(reported["policy"]["enabled"], true);
    let accepted = reported;
    assert_eq!(accepted["clients"][0]["review"], "accepted");
    assert_eq!(accepted["configuration"]["id"], program.configuration.id);
    assert_eq!(accepted["filter_names"]["ha"], "H-alpha");

    let (status, _) = call(
        &app,
        "DELETE",
        &format!("/rigs/{rig}/clients/{client}"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let removed = view(&app).await;
    assert_eq!(removed["clients"], json!([]));
    // Reported equipment outlives the plugin that reported it.
    assert_eq!(removed["configuration"]["id"], program.configuration.id);
}

#[tokio::test]
async fn a_plugin_cannot_read_the_view_or_close_work() {
    let (dir, state, app, instance, _, _) = fixture().await;
    let (catalog, rig) = register(&dir, &state);
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    let close = format!("/rigs/{rig}/workloads/{}/close", Uuid::new_v4());
    let body = json!({"coordinator_instance_id":instance,"catalog_id":catalog});
    assert_eq!(
        client_call(&app, "POST", &close, body.clone(), token, Some(profile))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        client_call(
            &app,
            "GET",
            "/catalogs/plugin/rig/plugin",
            Value::Null,
            token,
            Some(profile)
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    // An operator closing work the rig does not have finds nothing.
    assert_eq!(
        call(&app, "POST", &close, body, None).await.0,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn pairing_leaves_a_chosen_mode_and_fills_in_a_missing_one() {
    let (dir, state, app, instance, catalog, rig) = fixture().await;
    let (plugin_catalog, plugin_rig) = register(&dir, &state);
    let mode = |rig| {
        state
            .director
            .as_ref()
            .unwrap()
            .writer
            .lock()
            .unwrap()
            .rig_profile(rig)
            .unwrap()
            .unwrap()
            .mode
    };

    let mut chosen = RigProfile::empty(rig, 100);
    chosen.mode = Some(RigMode::TargetScheduler);
    state
        .director
        .as_ref()
        .unwrap()
        .writer
        .lock()
        .unwrap()
        .save_rig_profile(&chosen, 0)
        .unwrap();
    credential(&app, instance, catalog, rig, Uuid::new_v4()).await;
    assert_eq!(mode(rig), Some(RigMode::TargetScheduler));

    credential(&app, instance, plugin_catalog, plugin_rig, Uuid::new_v4()).await;
    assert_eq!(mode(plugin_rig), Some(RigMode::Director));
}
