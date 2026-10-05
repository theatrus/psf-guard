use super::*;
use psf_guard_director_meta::CatalogIdentity;

async fn login_cookie(state: Arc<AppState>, name: &str) -> String {
    let app = Router::new()
        .route("/login", axum::routing::post(auth::login))
        .with_state(state);
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"username":name,"password":"test-password-not-real"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.headers()["set-cookie"]
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .into()
}

#[tokio::test]
async fn only_interactive_editors_manage_clients_and_cookies_cannot_mask_wrong_tokens() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let mut registry = AuthRegistry::default();
    for (name, role) in [
        ("editor", AccessRole::ReadWrite),
        ("reader", AccessRole::ReadOnly),
    ] {
        registry
            .add(
                AuthUserRecord::new(name, role, "test-password-not-real").unwrap(),
                false,
            )
            .unwrap();
    }
    let (pat, record) = AuthTokenRecord::mint("editor", "operator", false, None).unwrap();
    registry.tokens.push(record);
    state.set_server_auth(auth::ServerAuth::from_sources(None, &registry, 3000).unwrap());
    let endpoint = format!("/rigs/{rig}/pairing-token");
    let input = json!({"coordinator_instance_id":instance,"catalog_id":catalog});
    assert_eq!(
        call(&app, "POST", &endpoint, input.clone(), Some(&pat))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    for (name, expected) in [
        ("reader", StatusCode::FORBIDDEN),
        ("editor", StatusCode::OK),
    ] {
        let cookie = login_cookie(state.clone(), name).await;
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/director/v1{endpoint}"))
                    .header("content-type", "application/json")
                    .header("cookie", &cookie)
                    .body(Body::from(input.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), expected);
        for token in ["psfrc_sync-key", "psfdrc_invalid", "psfg_invalid"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/director/v1/projects")
                        .header("cookie", &cookie)
                        .header("authorization", format!("Bearer {token}"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        }
    }
}

#[tokio::test]
async fn client_can_deliver_checkin_and_status_but_cannot_change_scope_in_payload() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    state.set_anonymous_access_trusted(false);
    let event = json!({"schema_version":1,"ledger_id":"ledger-1","sequence":1,"contract_version":2,"engine_version":"0.7.0",
        "assignment_id":"assignment-1","assignment_revision":1,"rig_id":rig,"configuration_id":"config-1",
        "attempt":{"capture_id":"capture-1","goal_id":"goal-1","reserved_at_ms":1,"evidence":{"state":"reserved"}}});
    let checkin = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"ledger_id":"ledger-1","events":[event]});
    let status = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"session_id":"session-1","reported_at_ms":1,"status":{"state":"idle"}});
    let ledger = Uuid::new_v4();
    let operations = json!({"coordinator_instance_id":instance,"catalog_id":catalog,"ledger_id":ledger,"events":[{
        "schema_version":1,"ledger_id":ledger,"sequence":1,"contract_version":2,"engine_version":"0.3.0",
        "assignment_id":"assignment-1","assignment_revision":1,"rig_id":rig,"configuration_id":"config-1",
        "preparation_id":"prep-1","event":{"kind":"closed"}
    }]});
    for (suffix, input) in [
        ("checkin", checkin),
        ("status", status),
        ("operations", operations),
    ] {
        let path = format!("/rigs/{rig}/{suffix}");
        let (code, body) =
            client_call(&app, "POST", &path, input.clone(), token, Some(profile)).await;
        assert_eq!(code, StatusCode::OK, "{body}");
        for field in ["coordinator_instance_id", "catalog_id"] {
            let mut wrong = input.clone();
            wrong[field] = json!(Uuid::new_v4());
            assert_eq!(
                client_call(&app, "POST", &path, wrong, token, Some(profile))
                    .await
                    .0,
                StatusCode::FORBIDDEN
            );
        }
    }
}

#[tokio::test]
async fn paired_rig_cannot_extend_another_rigs_ledger() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let first = credential(&app, instance, catalog, rig, profile).await;
    let other_catalog = Uuid::new_v4();
    let other_rig = state
        .director
        .clone()
        .unwrap()
        .run(move |store| {
            Ok(store
                .bind_catalog_rig_after(
                    CatalogIdentity {
                        id: other_catalog,
                        origin_instance_id: instance,
                    },
                    "Other rig",
                    true,
                    || Ok(()),
                )?
                .rig
                .id)
        })
        .await
        .unwrap();
    let other = credential(&app, instance, other_catalog, other_rig, profile).await;
    for (owner, source, token, sequence, expected) in [
        (
            rig,
            catalog,
            first["token"].as_str().unwrap(),
            1,
            StatusCode::OK,
        ),
        (
            other_rig,
            other_catalog,
            other["token"].as_str().unwrap(),
            2,
            StatusCode::CONFLICT,
        ),
    ] {
        let body = json!({"coordinator_instance_id":instance,"catalog_id":source,"ledger_id":"shared-ledger","events":[
            {"schema_version":1,"ledger_id":"shared-ledger","sequence":sequence,"contract_version":2,"engine_version":"0.7.0",
            "assignment_id":"assignment-1","assignment_revision":1,"rig_id":owner,"configuration_id":"config-1",
            "attempt":{"capture_id":"capture-1","goal_id":"goal-1","reserved_at_ms":1,"evidence":{"state":"reserved"}}}
        ]});
        let (code, body) = client_call(
            &app,
            "POST",
            &format!("/rigs/{owner}/checkin"),
            body,
            token,
            Some(profile),
        )
        .await;
        assert_eq!(code, expected, "{body}");
    }
    state
        .director
        .clone()
        .unwrap()
        .run(move |store| {
            assert_eq!(
                store
                    .feed_cursor(rig, "shared-ledger")?
                    .unwrap()
                    .highest_seen,
                1
            );
            assert!(store.feed_cursor(other_rig, "shared-ledger")?.is_none());
            Ok(())
        })
        .await
        .unwrap();
}

pub(super) async fn fixture() -> (TempDir, Arc<AppState>, Router, Uuid, Uuid, Uuid) {
    let dir = TempDir::new().unwrap();
    let state = Arc::new(state(&dir, true));
    let service = state.director.clone().unwrap();
    let instance = service.instance_id;
    let catalog = Uuid::new_v4();
    let rig = service
        .run(move |store| {
            Ok(store
                .bind_catalog_rig_after(
                    CatalogIdentity {
                        id: catalog,
                        origin_instance_id: instance,
                    },
                    "Rig",
                    true,
                    || Ok(()),
                )?
                .rig
                .id)
        })
        .await
        .unwrap();
    (dir, state.clone(), router(state), instance, catalog, rig)
}

pub(super) async fn credential(
    app: &Router,
    instance: Uuid,
    catalog: Uuid,
    rig: Uuid,
    profile: Uuid,
) -> Value {
    let (status, issued) = call(
        app,
        "POST",
        &format!("/rigs/{rig}/pairing-token"),
        json!({"coordinator_instance_id":instance,"catalog_id":catalog}),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{issued}");
    let (status,paired) = call(app,"POST","/pair",json!({"protocol_version":1,"pairing_token":issued["data"]["pairing_token"],"profile_id":profile,"client_name":"NINA"}),None).await;
    assert_eq!(status, StatusCode::OK, "{paired}");
    paired["data"].clone()
}

pub(super) async fn client_call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    token: &str,
    profile: Option<Uuid>,
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(format!("/api/director/v1{path}"))
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"));
    if let Some(profile) = profile {
        req = req.header("x-psf-director-profile", profile.to_string());
    }
    let response = app
        .clone()
        .oneshot(req.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let status = response.status();
    let body = to_bytes(response.into_body(), 100_000).await.unwrap();
    (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
}

#[tokio::test]
async fn pairing_wire_single_use_no_secrets_in_listing_and_revocation() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let (_, issued) = call(
        &app,
        "POST",
        &format!("/rigs/{rig}/pairing-token"),
        json!({"coordinator_instance_id":instance,"catalog_id":catalog}),
        None,
    )
    .await;
    let code = issued["data"]["pairing_token"].as_str().unwrap();
    assert!(code.starts_with("psfdpt_"));
    let input = json!({"protocol_version":1,"pairing_token":code,"profile_id":profile,"client_name":"NINA"});
    // Pairing works on an account-less network bind only by the one-use code.
    state.set_anonymous_access_trusted(false);
    let (status, paired) = call(&app, "POST", "/pair", input.clone(), None).await;
    assert_eq!(status, StatusCode::OK);
    let data = &paired["data"];
    assert_eq!(data["protocol_version"], 1);
    assert_eq!(data["coordinator_instance_id"], instance.to_string());
    assert_eq!(data["catalog_id"], catalog.to_string());
    assert_eq!(data["rig_id"], rig.to_string());
    assert_eq!(data["profile_id"], profile.to_string());
    assert_eq!(
        data["scopes"],
        json!(["program:read", "checkin:write", "status:write"])
    );
    let token = data["token"].as_str().unwrap();
    assert!(token.starts_with("psfdrc_"));
    assert_eq!(
        call(&app, "POST", "/pair", input, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    state.set_anonymous_access_trusted(true);
    let (_, list) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/clients"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(list["data"][0]["client_id"], data["client_id"]);
    assert!(!list.to_string().contains(token));
    assert!(!list.to_string().contains(code));
    assert!(!list.to_string().contains("token_hash"));
    let path =
        format!("/rigs/{rig}/program?coordinator_instance_id={instance}&catalog_id={catalog}");
    // Authorized reaches the handler (no equipment/profile configured yet).
    assert_ne!(
        client_call(&app, "GET", &path, Value::Null, token, Some(profile))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        call(
            &app,
            "DELETE",
            &format!(
                "/rigs/{rig}/clients/{}",
                data["client_id"].as_str().unwrap()
            ),
            Value::Null,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        client_call(&app, "GET", &path, Value::Null, token, Some(profile))
            .await
            .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn client_scope_profile_rig_and_token_class_are_exact_even_on_loopback() {
    let (_dir, state, app, instance, catalog, rig) = fixture().await;
    let profile = Uuid::new_v4();
    let paired = credential(&app, instance, catalog, rig, profile).await;
    let token = paired["token"].as_str().unwrap();
    for (method, path) in [
        ("GET", "/projects".into()),
        ("GET", "/rigs/status".into()),
        ("PUT", format!("/rigs/{rig}/equipment")),
        ("POST", format!("/rigs/{rig}/pairing-token")),
        ("GET", format!("/rigs/{rig}/clients")),
        ("GET", format!("/rigs/{rig}/status")),
        ("HEAD", format!("/rigs/{rig}/program")),
        ("GET", format!("/rigs/{}/program", Uuid::new_v4())),
        ("POST", "/pair".into()),
    ] {
        assert_eq!(
            client_call(&app, method, &path, json!({}), token, Some(profile))
                .await
                .0,
            StatusCode::UNAUTHORIZED,
            "{method} {path}"
        );
    }
    let path =
        format!("/rigs/{rig}/program?coordinator_instance_id={instance}&catalog_id={catalog}");
    for bad_profile in [None, Some(Uuid::new_v4())] {
        assert_eq!(
            client_call(&app, "GET", &path, Value::Null, token, bad_profile)
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    for bad_token in ["psfrc_sync-key", "psfpt_sync-pairing", "arbitrary"] {
        assert_eq!(
            client_call(&app, "GET", &path, Value::Null, bad_token, Some(profile))
                .await
                .0,
            StatusCode::UNAUTHORIZED
        );
    }
    // A paired rig reads its program whatever the server's management flag;
    // the answer is the same either way.
    let open = client_call(&app, "GET", &path, Value::Null, token, Some(profile))
        .await
        .0;
    state.set_allow_database_management(false);
    assert_eq!(
        client_call(&app, "GET", &path, Value::Null, token, Some(profile))
            .await
            .0,
        open
    );
}

#[tokio::test]
async fn wrong_coordinator_catalog_pairing_and_bounded_input_cannot_mint_clients() {
    let (_dir, _state, app, instance, catalog, rig) = fixture().await;
    for input in [
        json!({"coordinator_instance_id":Uuid::new_v4(),"catalog_id":catalog}),
        json!({"coordinator_instance_id":instance,"catalog_id":Uuid::new_v4()}),
    ] {
        assert_ne!(
            call(
                &app,
                "POST",
                &format!("/rigs/{rig}/pairing-token"),
                input,
                None
            )
            .await
            .0,
            StatusCode::OK
        );
    }
    assert_eq!(call(&app,"POST","/pair",json!({"protocol_version":1,"pairing_token":"x".repeat(5000),"profile_id":Uuid::new_v4(),"client_name":"NINA"}),None).await.0,StatusCode::PAYLOAD_TOO_LARGE);
    let (_, list) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/clients"),
        Value::Null,
        None,
    )
    .await;
    assert_eq!(list["data"], json!([]));
    let secret_marker = "psfdpt_secret-in-wrong-field";
    for body in [
        json!({"protocol_version":secret_marker,"pairing_token":"unused","profile_id":Uuid::new_v4(),"client_name":"NINA"}),
        json!({secret_marker:"unused"}),
    ] {
        let (status, error) = call(&app, "POST", "/pair", body, None).await;
        assert!(status.is_client_error());
        assert!(!error.to_string().contains(secret_marker));
    }
}
