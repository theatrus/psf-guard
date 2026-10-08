use super::*;
use psf_guard_director_core::visibility::Site;
use psf_guard_director_meta::profile::{Reported, Source as ProfileSource};

async fn locate(state: &Arc<AppState>, rig: Uuid) {
    state
        .director
        .clone()
        .unwrap()
        .run(move |s| {
            let mut p = s.rig_profile(rig)?.unwrap();
            let revision = p.revision;
            p.site = Some(Reported {
                value: Site {
                    latitude_degrees: 35.0,
                    longitude_degrees: -105.0,
                    elevation_meters: 2000.0,
                },
                source: ProfileSource::Manual {},
                reported_at_ms: 1000,
            });
            s.save_rig_profile(&p, revision)?;
            Ok(())
        })
        .await
        .unwrap();
}

async fn catalog(
    dir: &std::path::Path,
    state: &Arc<AppState>,
    app: &Router,
) -> (Uuid, Uuid, rusqlite::Connection) {
    let path = dir.join("rig.sqlite");
    let db = crate::ts_schema::create_fresh_db(&path).unwrap();
    for exposure in [120, 300] {
        db.execute("INSERT INTO exposuretemplate (profileId,name,filtername,gain,offset,bin,readoutmode,twilightlevel,moonavoidanceenabled,moonavoidanceseparation,moonavoidancewidth,maximumhumidity,defaultexposure,moonrelaxscale,moonrelaxmaxaltitude,moonrelaxminaltitude,moondownenabled,ditherevery,minutesOffset,guid) VALUES ('profile',?1,'OIII',100,30,1,-1,0,1,60,7,0,?2,0,5,-15,0,3,0,?3)", rusqlite::params![format!("O {exposure}"), exposure, Uuid::new_v4().to_string()]).unwrap();
    }
    let context = DatabaseContext::new(
        "rig-db".into(),
        "Rig database".into(),
        path.to_string_lossy().into(),
        vec![dir.to_string_lossy().into()],
        None,
        None,
        None,
        dir.join("cache-rig"),
    )
    .unwrap();
    state
        .databases
        .write()
        .unwrap()
        .insert("rig-db".into(), Arc::new(context));
    let catalog = Uuid::new_v4();
    let (status, preview) = call(
        app,
        "POST",
        "/catalogs/rig-db/rig/preview",
        json!({"catalog_id":catalog}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status, applied) = call(
        app,
        "POST",
        "/catalogs/rig-db/rig/apply",
        json!({"plan":{"catalog_id":catalog},"preview_digest":preview["data"]["preview_digest"]}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    let rig = serde_json::from_value(applied["data"]["binding"]["rig"]["id"].clone()).unwrap();
    (rig, catalog, db)
}

fn wire() -> Value {
    let mut wire: Value = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/director-interop/tests/fixtures/starfront-tonight.json"
    )))
    .unwrap();
    wire.as_object_mut().unwrap().remove("task");
    wire
}

#[tokio::test]
async fn background_status_does_not_wait_for_the_network_gate() {
    let (_dir, state, app, rig) = fixture().await;
    let id = add(&app, rig, "http://127.0.0.1:1/", &[]).await;
    let service = state.director.clone().unwrap();
    let _gate = service.collaboration.gate.lock().await;
    let (status, reply) = tokio::time::timeout(
        Duration::from_secs(2),
        call(
            &app,
            "POST",
            &format!("/collaboration/{id}/work"),
            json!({"operation":"background_status"}),
            &[],
        ),
    )
    .await
    .expect("Status must stay responsive during a remote request");
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert!(reply["data"]["policy"].is_null());
}

#[tokio::test]
async fn tonight_resolves_site_context_and_background_activation_is_scoped_idempotent_and_offline_safe(
) {
    let (dir, state, app, _) = fixture().await;
    let (rig, catalog, db) = catalog(dir.path(), &state, &app).await;
    commissioned(&state, rig).await;
    let reads = Arc::new(AtomicUsize::new(0));
    let failing = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shared = Arc::new(Mutex::new(wire()));
    let (r, f, w) = (reads.clone(), failing.clone(), shared.clone());
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async { Json(health(json!(["pairing"]))) }),
            )
            .route(
                "/api/v1/pair",
                axum::routing::post(|| async {
                    Json(json!({"agent":{"id":"000000000001"},"token":"test-token"}))
                }),
            )
            .route(
                "/api/v1/agent/hello",
                axum::routing::post(|| async {
                    Json(json!({"agent":"000000000001","protocol":1,"serverTime":1791171023.0}))
                }),
            )
            .route(
                "/api/v1/agent/projects",
                get(|| async {
                    let mut projects: Value = serde_json::from_slice(include_bytes!(concat!(
                        env!("CARGO_MANIFEST_DIR"),
                        "/crates/director-interop/tests/fixtures/starfront-projects.json"
                    )))
                    .unwrap();
                    for project in projects["projects"].as_array_mut().unwrap() {
                        project["joined"] = json!(project["id"] == "000000000002");
                    }
                    Json(projects)
                }),
            )
            .route(
                "/api/v1/agent/task",
                get(move |Query(query): Query<HashMap<String, String>>| {
                    let (r, f, w) = (r.clone(), f.clone(), w.clone());
                    async move {
                        r.fetch_add(1, Ordering::SeqCst);
                        if f.load(Ordering::SeqCst) {
                            return StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        for key in ["moon", "moonUp"] {
                            assert!((0.0..=1.0).contains(&query[key].parse::<f64>().unwrap()));
                        }
                        let mut value = w.lock().unwrap().clone();
                        for task in value["tasks"].as_array_mut().unwrap() {
                            task["assignedNight"] = json!(query["night"]);
                        }
                        Json(value).into_response()
                    }
                }),
            ),
    )
    .await;
    let id = add(&app, rig, &endpoint, &[]).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "pair"),
            json!({"code":"PAIR"}),
            &[]
        )
        .await
        .0,
        StatusCode::OK
    );
    let route = format!("/collaboration/{id}/work");
    assert_eq!(
        call(&app, "POST", &route, setup_filters(), &[]).await.0,
        StatusCode::OK
    );
    let (status, error) = call(&app, "POST", &route, json!({"operation":"tonight"}), &[]).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{error}");
    assert_eq!(reads.load(Ordering::SeqCst), 0);
    locate(&state, rig).await;
    let (status, work) = call(&app, "POST", &route, json!({"operation":"tonight"}), &[]).await;
    assert_eq!(status, StatusCode::OK, "{work}");
    let night = work["data"]["night"].clone();
    let expected = workflows::resolve_night(state.director.clone().unwrap(), rig, None, None)
        .await
        .unwrap();
    assert_eq!(night, serde_json::to_value(expected).unwrap());
    let (status, override_work) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"tonight","observing_date":"2026-10-05"}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{override_work}");
    assert_eq!(override_work["data"]["night"]["night"], "2026-10-05");
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"tonight","observing_date":"2026-02-30"}),
            &[]
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    // Allow a joined project before the first manual import, without enrolling
    // or joining anything in the background.
    let early_policy = json!({"enabled":true,"catalog_id":catalog,"project_ids":["000000000002"],"interval_minutes":5,"activate":false});
    let (status, early) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_configure","expected":null,"policy":early_policy}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{early}");
    let (status, pulled) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_run"}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{pulled}");
    assert_eq!(pulled["data"]["status"]["result"]["imported"], 1);
    assert_eq!(pulled["data"]["status"]["result"]["activated"], 0);
    assert_eq!(
        db.query_row("SELECT COUNT(*) FROM exposureplan", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        0
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"background_configure","expected":early_policy,"policy":null}),
            &[]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (status, preview) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"preview","task":"000000000004","night":night}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status,imported) = call(&app,"POST",&route,json!({"operation":"apply","task":"000000000004","night":night,"review_digest":preview["data"]["preview"]["review_digest"]}),&[]).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    let project: Uuid =
        serde_json::from_value(imported["data"]["plan"]["project_id"].clone()).unwrap();
    let scalar = |sql: &str| db.query_row(sql, [], |r| r.get::<_, i64>(0)).unwrap();
    assert_eq!(scalar("SELECT COUNT(*) FROM exposureplan"), 0);
    let before = reads.load(Ordering::SeqCst);
    super::super::background::sweep(&state).await.unwrap();
    assert_eq!(reads.load(Ordering::SeqCst), before); // off until explicitly configured
    let mut policy = json!({"enabled":true,"catalog_id":Uuid::new_v4(),"project_ids":["000000000002"],"interval_minutes":5,"activate":true});
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"background_configure","expected":null,"policy":policy}),
            &[]
        )
        .await
        .0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    policy["catalog_id"] = json!(catalog);
    let (status, saved) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_configure","expected":null,"policy":policy}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["data"]["projects"][0]["id"], "000000000002");
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"background_configure","expected":null,"policy":policy}),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let mut unrelated = shared.lock().unwrap()["tasks"][0].clone();
    unrelated["id"] = json!("000000000005");
    unrelated["project"] = json!("000000000003");
    shared.lock().unwrap()["tasks"]
        .as_array_mut()
        .unwrap()
        .push(unrelated);
    db.execute("INSERT INTO exposuretemplate (profileId,name,filtername,gain,offset,bin,readoutmode,twilightlevel,moonavoidanceenabled,moonavoidanceseparation,moonavoidancewidth,maximumhumidity,defaultexposure,moonrelaxscale,moonrelaxmaxaltitude,moonrelaxminaltitude,moondownenabled,ditherevery,minutesOffset,guid) VALUES ('profile','Ambiguous','OIII',100,30,1,-1,0,1,60,7,0,300,0,5,-15,0,3,0,?1)",[Uuid::new_v4().to_string()]).unwrap();
    super::super::background::sweep(&state).await.unwrap();
    let (_, ambiguous) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_status"}),
        &[],
    )
    .await;
    assert_eq!(
        ambiguous["data"]["status"]["result"]["activated"], 0,
        "{ambiguous}"
    );
    assert!(!ambiguous["data"]["status"]["result"]["held"]
        .as_array()
        .unwrap()
        .is_empty());
    assert_eq!(scalar("SELECT COUNT(*) FROM exposureplan"), 0);
    db.execute("DELETE FROM exposuretemplate WHERE name='Ambiguous'", [])
        .unwrap();
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"background_run"}),
            &[]
        )
        .await
        .0,
        StatusCode::OK
    );
    let (_, status) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_status"}),
        &[],
    )
    .await;
    assert_eq!(
        status["data"]["status"]["result"]["activated"], 1,
        "{status}"
    );
    assert_eq!(
        status["data"]["status"]["result"]["held"],
        json!([]),
        "{status}"
    );
    assert_eq!(scalar("SELECT COUNT(*) FROM exposureplan"), 6);
    assert_eq!(scalar("SELECT MIN(desired) FROM exposureplan"), 11);
    let b = binding(state.director.clone().unwrap(), id).await.unwrap();
    let credential = registry(&state).unwrap();
    credentials::write(&credential, &b, None).unwrap();
    let before_missing = reads.load(Ordering::SeqCst);
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"background_run"}),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(reads.load(Ordering::SeqCst), before_missing);
    assert_eq!(scalar("SELECT MIN(desired) FROM exposureplan"), 11);
    let (_, missing) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_status"}),
        &[],
    )
    .await;
    assert_eq!(missing["data"]["connection_status"], "credential_missing");
    credentials::write(&credential, &b, Some("test-token")).unwrap();
    assert_eq!(scalar("SELECT COUNT(*) FROM project"), 1); // allowlist excludes other project
    assert_eq!(
        scalar("SELECT MIN(moonavoidanceenabled) FROM exposuretemplate WHERE name='O 300'"),
        1
    );
    db.execute("UPDATE exposureplan SET acquired=1,accepted=1", [])
        .unwrap();
    let (status, repeat) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_run"}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{repeat}");
    assert_eq!(repeat["data"]["status"]["result"]["activated"], 0);
    assert_eq!(scalar("SELECT MIN(desired) FROM exposureplan"), 11);
    failing.store(true, Ordering::SeqCst);
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"background_run"}),
            &[]
        )
        .await
        .0,
        StatusCode::BAD_GATEWAY
    );
    let (_, held) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_status"}),
        &[],
    )
    .await;
    assert!(held["data"]["status"]["last_error"].is_string());
    assert_eq!(held["data"]["status"]["running"], false);
    assert_eq!(scalar("SELECT MIN(desired) FROM exposureplan"), 11);
    failing.store(false, Ordering::SeqCst);
    shared.lock().unwrap()["tasks"][0]["version"] = json!(3);
    shared.lock().unwrap()["tasks"][0]["visit"]["frames"]["O"] = json!(13);
    shared.lock().unwrap()["tasks"][0]["visit"]["seconds"] = json!(3900.0);
    let (status, new_work) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"background_run"}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{new_work}");
    assert_eq!(
        new_work["data"]["status"]["result"]["activated"], 1,
        "{new_work}"
    );
    assert_eq!(scalar("SELECT MIN(desired) FROM exposureplan"), 14);
    assert_eq!(scalar("SELECT MIN(accepted) FROM exposureplan"), 1);
    let draft = state
        .director
        .clone()
        .unwrap()
        .query(move |s| s.plan_draft(project))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(draft.contributions.len(), 1);
    assert_eq!(draft.contributions[0].template.name, "O 300");
    remote.abort();
}
