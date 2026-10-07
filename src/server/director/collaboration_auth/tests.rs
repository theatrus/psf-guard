use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
    middleware,
};
use std::sync::atomic::{AtomicUsize, Ordering};
use tower::ServiceExt;

async fn fixture() -> (tempfile::TempDir, Arc<AppState>, Router, Uuid) {
    let dir = tempfile::tempdir().unwrap();
    credentials::private_file::private_test_directory(dir.path()).unwrap();
    let mut state = AppState::from_databases(
        vec![],
        dir.path().join("cache"),
        crate::cli::PregenerationConfig::default(),
    )
    .unwrap();
    state.set_registry_path(Some(dir.path().join("config.json")));
    state.set_allow_database_management(true);
    state.director = Service::configured(Some(&dir.path().join("meta.sqlite"))).unwrap();
    let rig = Uuid::new_v4();
    state
        .director
        .clone()
        .unwrap()
        .run(move |s| s.create_rig(rig, "Rig"))
        .await
        .unwrap();
    let state = Arc::new(state);
    let app = Router::new()
        .nest("/api/director/v1", super::super::routes())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            auth::authorize_api,
        ))
        .with_state(state.clone());
    (dir, state, app, rig)
}
async fn call(
    app: &Router,
    method: &str,
    path: &str,
    body: Value,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut r = Request::builder()
        .method(method)
        .uri(format!("/api/director/v1{path}"))
        .header("content-type", "application/json");
    for (name, value) in headers {
        r = r.header(*name, *value);
    }
    let response = app
        .clone()
        .oneshot(r.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 100_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}
async fn mock(app: Router) -> (String, tokio::task::JoinHandle<()>) {
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = tcp.local_addr().unwrap();
    let task = tokio::spawn(async move {
        axum::serve(tcp, app).await.unwrap();
    });
    (format!("http://{addr}/"), task)
}
fn health(features: Value) -> Value {
    json!({"ok":true,"protocol":1,"version":"test","time":1791171023.0,"features":features})
}
async fn commissioned(state: &Arc<AppState>, rig: Uuid) {
    use psf_guard_director_core::optics::{Optics, Rotation};
    use psf_guard_director_meta::profile::{Reported, RigProfile, Source as ProfileSource};
    state
        .director
        .clone()
        .unwrap()
        .run(move |s| {
            let mut p = RigProfile::empty(rig, 1000);
            p.optics = Some(Reported {
                value: Optics {
                    sensor_width_px: 6248,
                    sensor_height_px: 4176,
                    pixel_size_um: 3.76,
                    focal_length_mm: 530.0,
                    aperture_mm: None,
                    rotation: Rotation::Rotator {},
                },
                source: ProfileSource::Manual {},
                reported_at_ms: 1000,
            });
            s.save_rig_profile(&p, 0)?;
            Ok(())
        })
        .await
        .unwrap();
}
fn setup_filters() -> Value {
    json!({"operation":"configure","settings":{"binning":1,"colour":false,"hours_per_night":6.0,"share_status":false,"filters":{"Ha":{"exposure_seconds":300.0,"bandpass_nm":7.0},"OIII":{"exposure_seconds":300.0,"bandpass_nm":7.0}}}})
}
fn observing_night() -> Value {
    json!({"night":"2026-10-05","moon":0.12,"moon_up":0.3})
}
#[tokio::test]
#[ignore = "Requires ASTROCOLLAB_REFERENCE and ASTROCOLLAB_PYTHON pointing to the public reference checkout and its Python"]
async fn reference_server_managed_pair_hello_join_and_reviewed_import() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    struct Reference(std::process::Child);
    impl Drop for Reference {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let root = std::env::var("ASTROCOLLAB_REFERENCE").expect("Set reference checkout path");
    let python = std::env::var("ASTROCOLLAB_PYTHON").unwrap_or("python".into());
    let mut process = Reference(
        Command::new(python)
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/support/astrocollab_reference.py"
            ))
            .arg(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    let mut line = String::new();
    BufReader::new(process.0.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let remote: Value = serde_json::from_str(&line).expect("Reference server did not start");
    let (_dir, state, app, rig) = fixture().await;
    commissioned(&state, rig).await;
    let id = add(&app, rig, remote["url"].as_str().unwrap(), &[]).await;
    let (status, paired) = call(
        &app,
        "POST",
        &action(id, "pair"),
        json!({"code":remote["code"]}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{paired}");
    let route = format!("/collaboration/{id}/work");
    assert_eq!(
        call(&app, "POST", &route, setup_filters(), &[]).await.0,
        StatusCode::OK
    );
    let (status, projects) = call(&app, "POST", &route, json!({"operation":"browse"}), &[]).await;
    assert_eq!(status, StatusCode::OK, "{projects}");
    let project = projects["data"]["projects"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["compatible"] == true)
        .expect("No compatible sample project")["project_id"]
        .clone();
    let (status, work) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"join","project":project,"night":observing_night()}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{work}");
    let share = work["data"]["shares"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["review_reasons"].as_array().unwrap().is_empty())
        .expect("Reference share requires review");
    let task = share["task_id"].clone();
    let (status, preview) = call(
        &app,
        "POST",
        &route,
        json!({"operation":"preview","task":task,"night":observing_night()}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let (status,imported)=call(&app,"POST",&route,json!({"operation":"apply","task":task,"night":observing_night(),"review_digest":preview["data"]["preview"]["review_digest"]}),&[]).await;
    assert_eq!(status, StatusCode::OK, "{imported}");
    assert_eq!(imported["data"]["preview"]["acquisition_enabled"], false);
}
#[tokio::test]
async fn malformed_report_reply_acknowledges_nothing_and_replay_uses_same_snapshot() {
    use psf_guard_director_interop::collaboration::*;
    let (_dir, state, app, rig) = fixture().await;
    commissioned(&state, rig).await;
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
    let received = sent.clone();
    let (endpoint,remote)=mock(Router::new()
        .route("/api/v1/health",get(||async{Json(health(json!(["pairing"]))) }))
        .route("/api/v1/pair",axum::routing::post(||async{Json(json!({"agent":{"id":"000000000001"},"token":"test-agent-token"}))}))
        .route("/api/v1/agent/hello",axum::routing::post(||async{Json(json!({"agent":"000000000001","protocol":1,"serverTime":1791171023.0}))}))
        .route("/api/v1/agent/report",axum::routing::post(move |Json(body):Json<Value>|{let seen=seen.clone();let received=received.clone();async move{
            received.lock().unwrap().push(body);
            if seen.fetch_add(1,Ordering::SeqCst)==0 {Json(json!({"recorded":[]}))}
            else {Json(json!({"recorded":[{"id":"000000000010","accepted":false,"duplicate":true,"verdict":{"accepted":false,"reasons":["quality evidence incomplete"],"unverified":[]}}]}))}
        }}))).await;
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
    let source = Source::new(&endpoint, "000000000001", true).unwrap();
    let key = source.clone();
    let queued = state
        .director
        .clone()
        .unwrap()
        .run(move |s| {
            let plan = prepare_import(
                include_bytes!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/crates/director-interop/tests/fixtures/starfront-tonight.json"
                )),
                &source,
                "2026-10-05",
                "000000000004",
            )
            .unwrap();
            let preview = s.preview_collaboration_import(&plan, rig)?;
            s.apply_collaboration_import(&plan, rig, &preview.review_digest, 1000)?;
            let frame = FrameEvidence {
                capture_id: Uuid::new_v4(),
                image_guid: Uuid::new_v4(),
                source_digest: plan.digest().into(),
                panel_index: 0,
                filter: "OIII".into(),
                exposure_ms: 300000,
                saved: true,
                accepted: true,
                finalized: true,
                image_fingerprint: "verified-test-frame".into(),
                solve_fingerprint: "verified-test-frame".into(),
                solved_footprint: MeasuredFootprint {
                    ra: 10.5,
                    dec: 41.0,
                    width: 1.25,
                    height: 0.8,
                    rotation: 0.0,
                },
                scale_arcsec: None,
                focal_length_mm: None,
                hfr_arcsec: None,
                guide_rms_arcsec: None,
                moon_illumination: None,
                moon_separation_degrees: None,
                calibrated: false,
                bandpass_nm: None,
                colour: false,
            };
            s.queue_collaboration_report(&finalize_contribution(&plan, &[frame]).unwrap(), 2000)
        })
        .await
        .unwrap();
    assert!(
        !call(&app, "POST", &route, json!({"operation":"checkin"}), &[])
            .await
            .0
            .is_success()
    );
    let pending = state
        .director
        .clone()
        .unwrap()
        .query(move |s| s.pending_collaboration_reports(&key, 200))
        .await
        .unwrap();
    assert_eq!(pending[0].id, queued.id);
    let (status, body) = call(&app, "POST", &route, json!({"operation":"checkin"}), &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["data"]["accepted"], 0);
    assert_eq!(body["data"]["rejected"], 1);
    let sent = sent.lock().unwrap();
    assert_eq!(sent.len(), 2);
    assert_eq!(sent[0], sent[1]);
    remote.abort();
}
#[tokio::test]
async fn work_describes_rig_joins_and_requires_current_review_before_import() {
    let (_dir, state, app, rig) = fixture().await;
    commissioned(&state, rig).await;
    let mut wire: Value = serde_json::from_slice(include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/crates/director-interop/tests/fixtures/starfront-tonight.json"
    )))
    .unwrap();
    wire.as_object_mut().unwrap().remove("task");
    let mutable = Arc::new(Mutex::new(wire));
    let calls = Arc::new(Mutex::new(Vec::<Value>::new()));
    let received = calls.clone();
    let response = mutable.clone();
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async { Json(health(json!(["pairing"]))) }),
            )
            .route(
                "/api/v1/pair",
                axum::routing::post(|| async {
                    Json(json!({"agent":{"id":"000000000001"},"token":"test-agent-token"}))
                }),
            )
            .route(
                "/api/v1/agent/hello",
                axum::routing::post(move |headers: HeaderMap, Json(body): Json<Value>| {
                    let received = received.clone();
                    async move {
                        assert_eq!(headers["authorization"], "Bearer test-agent-token");
                        received.lock().unwrap().push(body);
                        Json(json!({"agent":"000000000001","protocol":1,"serverTime":1791171023.0}))
                    }
                }),
            )
            .route(
                "/api/v1/agent/projects/000000000002/join",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    assert_eq!(body["night"], "2026-10-05");
                    assert_eq!(body["exposures"]["Ha"], 300.0);
                    Json(json!({"task":{}}))
                }),
            )
            .route(
                "/api/v1/agent/task",
                get(move |Query(query): Query<HashMap<String, String>>| {
                    let response = response.clone();
                    async move {
                        assert_eq!(query["night"], "2026-10-05");
                        assert_eq!(query["moon"], "0.12");
                        Json(response.lock().unwrap().clone())
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
    assert_eq!(
        call(
            &app,
            "POST",
            &route,
            json!({"operation":"join","project":"000000000002","night":observing_night()}),
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
        json!({"operation":"preview","task":"000000000004","night":observing_night()}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    assert_eq!(preview["data"]["preview"]["acquisition_enabled"], false);
    let digest = preview["data"]["preview"]["review_digest"].clone();
    let apply = json!({"operation":"apply","task":"000000000004","night":observing_night(),"review_digest":digest});
    mutable.lock().unwrap()["tasks"][0]["version"] = json!(3);
    assert_eq!(
        call(&app, "POST", &route, apply.clone(), &[]).await.0,
        StatusCode::CONFLICT
    );
    mutable.lock().unwrap()["tasks"][0]["version"] = json!(2);
    let (status, body) = call(&app, "POST", &route, apply, &[]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(!body.to_string().contains("test-agent-token"));
    let import: Uuid = serde_json::from_value(body["data"]["plan"]["import_id"].clone()).unwrap();
    let stored = state
        .director
        .clone()
        .unwrap()
        .query(move |s| s.collaboration_import(import))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.rig_id, rig);
    assert!(calls
        .lock()
        .unwrap()
        .iter()
        .all(|v| v["profile"]["focalLength"] == 530.0 && v["presence"].is_null()));
    remote.abort();
}
async fn add(app: &Router, rig: Uuid, endpoint: &str, headers: &[(&str, &str)]) -> Uuid {
    let id = Uuid::new_v4();
    let (status, body) = call(
        app,
        "POST",
        &format!("/rigs/{rig}/collaboration"),
        json!({"id":id,"server_url":endpoint,"name":"Rig","allow_loopback_http":true}),
        headers,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    id
}
fn action(id: Uuid, action: &str) -> String {
    format!("/collaboration/{id}/{action}")
}

async fn cookie(state: Arc<AppState>, username: &str) -> String {
    let response = Router::new()
        .route("/login", axum::routing::post(auth::login))
        .with_state(state)
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/login")
                .header("content-type", "application/json")
                .body(Body::from(
                    json!({"username":username,"password":"test-password-not-real"}).to_string(),
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
async fn local_sessions_own_pending_signin_and_readers_and_api_tokens_cannot_enroll() {
    use crate::auth_registry::{AccessRole, AuthRegistry, AuthTokenRecord, AuthUserRecord};
    let (_dir, state, app, rig) = fixture().await;
    let mut users = AuthRegistry::default();
    for (name, role) in [
        ("first", AccessRole::ReadWrite),
        ("second", AccessRole::ReadWrite),
        ("reader", AccessRole::ReadOnly),
    ] {
        users
            .add(
                AuthUserRecord::new(name, role, "test-password-not-real").unwrap(),
                false,
            )
            .unwrap();
    }
    let (pat, record) = AuthTokenRecord::mint("first", "automation", false, None).unwrap();
    users.tokens.push(record);
    state.set_server_auth(auth::ServerAuth::from_sources(None, &users, 3000).unwrap());
    let first = cookie(state.clone(), "first").await;
    let second = cookie(state.clone(), "second").await;
    let reader = cookie(state.clone(), "reader").await;
    let bearer = format!("Bearer {pat}");
    for headers in [
        vec![("cookie", reader.as_str())],
        vec![("authorization", bearer.as_str())],
    ] {
        let (status, _) = call(
            &app,
            "POST",
            &format!("/rigs/{rig}/collaboration"),
            json!({"id":Uuid::new_v4(),"server_url":"https://example.com/","name":"Rig"}),
            &headers,
        )
        .await;
        assert_eq!(status, StatusCode::FORBIDDEN);
    }
    let id = add(&app, rig, "https://example.com/", &[("cookie", &first)]).await;
    let owner = auth::setup_owner(
        &state,
        &HeaderMap::from_iter([("cookie".parse().unwrap(), first.parse().unwrap())]),
    )
    .unwrap();
    let service = state.director.as_ref().unwrap();
    pending_insert(
        service,
        id,
        Pending {
            owner,
            code: "never-send".into(),
            expires: Instant::now() + Duration::from_secs(300),
            next_poll: Instant::now(),
        },
    )
    .unwrap();
    for command in ["poll", "cancel", "signin", "pair"] {
        let (status, _) = call(
            &app,
            "POST",
            &action(id, command),
            json!({}),
            &[("cookie", &second)],
        )
        .await;
        assert!(matches!(
            status,
            StatusCode::FORBIDDEN | StatusCode::CONFLICT
        ));
        assert!(service
            .collaboration
            .pending
            .lock()
            .unwrap()
            .contains_key(&id));
    }
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "cancel"),
            json!({}),
            &[("cookie", &first)]
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn remote_rejection_does_not_expire_local_session_and_storage_failure_retains_agent() {
    let (dir, state, app, rig) = fixture().await;
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async { Json(health(json!(["pairing"]))) }),
            )
            .route(
                "/api/v1/pair",
                axum::routing::post(|| async {
                    Json(json!({"agent":{"id":"000000000001"},"token":"secret"}))
                }),
            )
            .route(
                "/api/v1/agent/projects",
                get(|| async { StatusCode::UNAUTHORIZED }),
            ),
    )
    .await;
    let id = add(&app, rig, &endpoint, &[]).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "pair"),
            json!({"code":"once"}),
            &[]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        call(&app, "POST", &action(id, "validate"), json!({}), &[])
            .await
            .0,
        StatusCode::CONFLICT
    );
    let (_, reply) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/collaboration"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(reply["data"][0]["status"], "reauth_required");
    remote.abort();
    let p = dir.path().join("collaboration-credentials.json");
    std::fs::remove_file(&p).unwrap();
    let bad_file = p.clone();
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async { Json(health(json!(["pairing"]))) }),
            )
            .route(
                "/api/v1/pair",
                axum::routing::post(move || {
                    let p = bad_file.clone();
                    async move {
                        std::fs::write(p, "broken config").unwrap();
                        Json(json!({"agent":{"id":"000000000002"},"token":"one-shot"}))
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
            json!({"code":"once"}),
            &[]
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
    assert_eq!(
        binding(state.director.clone().unwrap(), id)
            .await
            .unwrap()
            .agent_id
            .as_deref(),
        Some("000000000002")
    );
    remote.abort();
}

#[tokio::test]
async fn pair_persists_without_leaking_and_missing_file_keeps_original_identity() {
    let (dir, state, app, rig) = fixture().await;
    let count = Arc::new(AtomicUsize::new(0));
    let pairs = count.clone();
    let (endpoint,remote) = mock(Router::new()
        .route("/api/v1/health",get(|| async { Json(health(json!(["pairing"]))) }))
        .route("/api/v1/pair",axum::routing::post(move |headers: HeaderMap, Json(body): Json<Value>| {
            let pairs = pairs.clone(); async move {
                assert!(!headers.contains_key("authorization")); assert_eq!(body["code"],"single-use");
                let n = pairs.fetch_add(1,Ordering::SeqCst)+1;
                Json(json!({"agent":{"id":format!("{n:012x}")},"token":"SECRET_AGENT_TOKEN"}))
            }
        }))
        .route("/api/v1/agent/projects",get(|headers: HeaderMap| async move {
            assert_eq!(headers["authorization"],"Bearer SECRET_AGENT_TOKEN"); Json(json!({"protocol":1,"projects":[]}))
        }))).await;
    let id = add(&app, rig, &endpoint, &[]).await;
    let (status, reply) = call(
        &app,
        "POST",
        &action(id, "pair"),
        json!({"code":"single-use"}),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["data"]["status"], "registered");
    assert!(!reply.to_string().contains("SECRET"));
    assert_eq!(
        call(&app, "POST", &action(id, "validate"), json!({}), &[])
            .await
            .0,
        StatusCode::OK
    );
    let p = dir.path().join("collaboration-credentials.json");
    assert!(std::fs::read_to_string(&p)
        .unwrap()
        .contains("SECRET_AGENT_TOKEN"));
    std::fs::remove_file(&p).unwrap();
    let (_, reply) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/collaboration"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(reply["data"][0]["status"], "credential_missing");
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "pair"),
            json!({"code":"single-use"}),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let original = binding(state.director.clone().unwrap(), id).await.unwrap();
    assert_eq!(original.agent_id.as_deref(), Some("000000000001"));
    let new = add(&app, rig, &endpoint, &[]).await;
    assert_eq!(
        call(
            &app,
            "POST",
            &action(new, "pair"),
            json!({"code":"single-use"}),
            &[]
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        binding(state.director.clone().unwrap(), id).await.unwrap(),
        original
    );
    remote.abort();
}

#[tokio::test]
async fn ambiguous_reply_never_retries_or_changes_agent() {
    let (_dir, _state, app, rig) = fixture().await;
    let count = Arc::new(AtomicUsize::new(0));
    let pairs = count.clone();
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async { Json(health(json!(["pairing"]))) }),
            )
            .route(
                "/api/v1/pair",
                axum::routing::post(move || {
                    let pairs = pairs.clone();
                    async move {
                        pairs.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"agent":{"id":"invalid"},"token":"secret"}))
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
            json!({"code":"once"}),
            &[]
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "pair"),
            json!({"code":"once"}),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    let (_, reply) = call(
        &app,
        "GET",
        &format!("/rigs/{rig}/collaboration"),
        Value::Null,
        &[],
    )
    .await;
    assert_eq!(reply["data"][0]["status"], "outcome_unknown");
    assert_eq!(count.load(Ordering::SeqCst), 1);
    remote.abort();
}

#[tokio::test]
async fn rejected_code_can_be_reentered_without_recreating_the_local_binding() {
    let (_dir, _state, app, rig) = fixture().await;
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async { Json(health(json!(["pairing"]))) }),
            )
            .route(
                "/api/v1/pair",
                axum::routing::post(|Json(body): Json<Value>| async move {
                    if body["code"] == "bad" {
                        StatusCode::UNAUTHORIZED.into_response()
                    } else {
                        Json(json!({"agent":{"id":"000000000001"},"token":"secret"}))
                            .into_response()
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
            json!({"code":"bad"}),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "pair"),
            json!({"code":"good"}),
            &[]
        )
        .await
        .0,
        StatusCode::OK
    );
    remote.abort();
}

#[tokio::test]
async fn discovery_and_redirects_never_guess_pairing_or_follow_bearer() {
    let (_dir, _state, app, rig) = fixture().await;
    let (endpoint, remote) = mock(
        Router::new()
            .route(
                "/api/v1/health",
                get(|| async {
                    Json(json!({"ok":true,"protocol":1,"version":"test","time":1791171023.0}))
                }),
            )
            .route(
                "/api/v1/auth",
                get(|| async { Json(json!({"discord":true})) }),
            )
            .route(
                "/api/v1/auth/login",
                axum::routing::post(|| async {
                    axum::response::Redirect::temporary("https://foreign.invalid/")
                }),
            ),
    )
    .await;
    let id = add(&app, rig, &endpoint, &[]).await;
    let (_, reply) = call(&app, "POST", &action(id, "discover"), json!({}), &[]).await;
    assert_eq!(reply["data"], json!({"signin":true,"pairing":false}));
    assert_eq!(
        call(
            &app,
            "POST",
            &action(id, "pair"),
            json!({"code":"never"}),
            &[]
        )
        .await
        .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "POST", &action(id, "signin"), json!({}), &[])
            .await
            .0,
        StatusCode::BAD_GATEWAY
    );
    remote.abort();
}

#[tokio::test]
async fn management_and_cross_origin_gates_precede_network_or_storage() {
    let (_dir, state, app, rig) = fixture().await;
    let input = json!({"id":Uuid::new_v4(),"server_url":"http://127.0.0.1:1","name":"Rig","allow_loopback_http":true});
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{rig}/collaboration"),
            input.clone(),
            &[("host", "127.0.0.1"), ("origin", "https://evil.invalid")]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    state.set_allow_database_management(false);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{rig}/collaboration"),
            input.clone(),
            &[]
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    state.set_allow_database_management(true);
    let mut input = input;
    input["allow_loopback_http"] = json!(false);
    assert_eq!(
        call(
            &app,
            "POST",
            &format!("/rigs/{rig}/collaboration"),
            input,
            &[]
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[tokio::test]
async fn browser_flow_validates_origin_and_keeps_person_token_on_host() {
    let (dir, state, app, rig) = fixture().await;
    let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/", tcp.local_addr().unwrap());
    let browser = format!("{endpoint}auth/start?code=DEVICE_CODE");
    let browser_copy = browser.clone();
    let polls = Arc::new(AtomicUsize::new(0));
    let remote = tokio::spawn(async move {
        axum::serve(tcp,Router::new()
        .route("/api/v1/health",get(|| async { Json(health(json!(["signin"]))) }))
        .route("/api/v1/auth/login",axum::routing::post(move || { let browser = browser_copy.clone(); async move { Json(json!({"code":"DEVICE_CODE","url":browser,"expiresIn":300})) }}))
        .route("/api/v1/auth/poll",get(move |Query(query):Query<HashMap<String,String>>| { let polls = polls.clone(); async move {
            assert_eq!(query["code"],"DEVICE_CODE");
            if polls.fetch_add(1, Ordering::SeqCst) == 0 { return StatusCode::TOO_MANY_REQUESTS.into_response(); }
            Json(json!({"state":"done","token":"SECRET_PERSON_TOKEN"})).into_response()
        }}))
        .route("/api/v1/agents",axum::routing::post(|headers:HeaderMap,Json(body):Json<Value>| async move {
            assert_eq!(headers["authorization"],"Bearer SECRET_PERSON_TOKEN"); assert_eq!(body["name"],"Rig");
            Json(json!({"agent":{"id":"000000000001"},"token":"SECRET_AGENT_TOKEN"}))
        }))).await.unwrap();
    });
    let id = add(&app, rig, &endpoint, &[]).await;
    let (status, reply) = call(&app, "POST", &action(id, "signin"), json!({}), &[]).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["data"]["url"], browser);
    assert!(!reply.to_string().contains("SECRET_PERSON"));
    assert_eq!(
        call(&app, "POST", &action(id, "signin"), json!({}), &[])
            .await
            .0,
        StatusCode::CONFLICT
    );
    assert_eq!(
        call(&app, "POST", &action(id, "poll"), json!({}), &[])
            .await
            .0,
        StatusCode::TOO_MANY_REQUESTS
    );
    state
        .director
        .as_ref()
        .unwrap()
        .collaboration
        .pending
        .lock()
        .unwrap()
        .get_mut(&id)
        .unwrap()
        .next_poll = Instant::now();
    let (status, reply) = call(&app, "POST", &action(id, "poll"), json!({}), &[]).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS, "{reply}");
    state
        .director
        .as_ref()
        .unwrap()
        .collaboration
        .pending
        .lock()
        .unwrap()
        .get_mut(&id)
        .expect("remote rate limit must preserve pending approval")
        .next_poll = Instant::now();
    let (status, reply) = call(&app, "POST", &action(id, "poll"), json!({}), &[]).await;
    assert_eq!(status, StatusCode::OK, "{reply}");
    assert_eq!(reply["data"]["status"], "registered");
    assert!(!reply.to_string().contains("SECRET"));
    assert!(
        !std::fs::read_to_string(dir.path().join("collaboration-credentials.json"))
            .unwrap()
            .contains("SECRET_PERSON")
    );
    let b = binding(state.director.clone().unwrap(), id).await.unwrap();
    let client = Remote::new(&b).unwrap();
    assert!(client.browser_url("https://foreign.invalid/login").is_err());
    assert!(client
        .browser_url(&endpoint.replace("http://", "http://user:pass@"))
        .is_err());
    remote.abort();
}
