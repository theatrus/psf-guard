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
