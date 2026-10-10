//! The reject-removal routes in process: preview, apply (and a stale one),
//! the list of removals, restore, and the database-management gate.

use std::sync::Arc;

use axum::{
    body::Body,
    http::{Request, StatusCode},
    routing::{get, post},
    Router,
};
use http_body_util::BodyExt;
use psf_guard::server::{database_context::DatabaseContext, reject_removal, state::AppState};
use rusqlite::Connection;
use serde_json::{json, Value};
use tower::ServiceExt;

struct Server {
    _dir: tempfile::TempDir,
    root: std::path::PathBuf,
    state: Arc<AppState>,
    app: Router,
}

fn server() -> Server {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let light = root.join("images/M42/2026-10-01/LIGHT");
    std::fs::create_dir_all(&light).unwrap();
    std::fs::write(light.join("M42_Ha_001.fits"), vec![1u8; 4096]).unwrap();
    std::fs::write(light.join("M42_Ha_002.fits"), vec![2u8; 4096]).unwrap();
    let database = root.join("rig.sqlite");
    {
        let connection = Connection::open(&database).unwrap();
        psf_guard::ts_schema::apply_schema(&connection).unwrap();
        connection
            .execute_batch(
                r#"INSERT INTO project (Id, profileId, name, guid) VALUES (1, 'p', 'M42', 'pg');
                 INSERT INTO target (Id, name, active, epochcode, projectId, guid) VALUES (1, 'M42', 1, 0, 1, 'tg');
                 INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, guid) VALUES
                    (1, 1, 1, 1759300000, 'Ha', 2, '{"FileName":"M42_Ha_001.fits"}', 'bad-one'),
                    (2, 1, 1, 1759300300, 'Ha', 1, '{"FileName":"M42_Ha_002.fits"}', 'good-one');"#,
            )
            .unwrap();
    }
    let state = Arc::new(AppState::new_for_test(
        Connection::open_in_memory().unwrap(),
    ));
    state.set_allow_database_management(true);
    let context = DatabaseContext::new(
        "rig".into(),
        "Rig".into(),
        database.display().to_string(),
        vec![root.join("images").display().to_string()],
        None,
        None,
        None,
        root.join("cache"),
    )
    .unwrap();
    state
        .databases
        .write()
        .unwrap()
        .insert("rig".into(), Arc::new(context));
    let app = Router::new()
        .route(
            "/api/db/{db_id}/rejects/removal/preview",
            post(reject_removal::preview),
        )
        .route(
            "/api/db/{db_id}/rejects/removal/apply",
            post(reject_removal::apply),
        )
        .route(
            "/api/db/{db_id}/rejects/removed",
            get(reject_removal::removed),
        )
        .route(
            "/api/db/{db_id}/rejects/removed/restore",
            post(reject_removal::restore),
        )
        .route(
            "/api/db/{db_id}/rejects/trash/empty",
            post(reject_removal::empty_trash),
        )
        .with_state(state.clone());
    Server {
        _dir: dir,
        root,
        state,
        app,
    }
}

async fn call(app: &Router, method: &str, uri: &str, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    let body = if body.is_null() {
        Body::empty()
    } else {
        request = request.header("content-type", "application/json");
        Body::from(body.to_string())
    };
    let response = app
        .clone()
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn rejects_are_previewed_removed_listed_and_restored_over_http() {
    let s = server();
    // A cache file the removed frame's row Id names.
    let solve = s.root.join("cache/rig/astrometry/1.json");
    std::fs::create_dir_all(solve.parent().unwrap()).unwrap();
    std::fs::write(&solve, b"{}").unwrap();

    let (status, preview) = call(
        &s.app,
        "POST",
        "/api/db/rig/rejects/removal/preview",
        json!({"min_age_days": 0}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{preview}");
    let data = &preview["data"];
    assert_eq!(data["frames"].as_array().unwrap().len(), 1, "{preview}");
    assert_eq!(data["frames"][0]["guid"], "bad-one");
    assert_eq!(data["bytes"], 4096);
    let digest = data["digest"].as_str().unwrap().to_owned();

    // A stale digest is refused and nothing changes.
    let (status, refused) = call(
        &s.app,
        "POST",
        "/api/db/rig/rejects/removal/apply",
        json!({"min_age_days": 0, "digest": "0".repeat(64)}),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{refused}");

    let (status, applied) = call(
        &s.app,
        "POST",
        "/api/db/rig/rejects/removal/apply",
        json!({"min_age_days": 0, "digest": digest}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{applied}");
    assert_eq!(applied["data"]["removed"].as_array().unwrap().len(), 1);
    assert!(!s
        .root
        .join("images/M42/2026-10-01/LIGHT/M42_Ha_001.fits")
        .exists());
    assert!(
        !solve.exists(),
        "the removed frame's cache files go with it"
    );

    let (status, listed) = call(&s.app, "GET", "/api/db/rig/rejects/removed", Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{listed}");
    let batch = listed["data"]["batches"][0]["batch_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (_, frames) = call(
        &s.app,
        "GET",
        &format!("/api/db/rig/rejects/removed?batch={batch}"),
        Value::Null,
    )
    .await;
    assert_eq!(frames["data"]["frames"][0]["guid"], "bad-one");

    let (status, emptied) = call(
        &s.app,
        "POST",
        "/api/db/rig/rejects/trash/empty",
        Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{emptied}");
    assert_eq!(
        emptied["data"]["files_deleted"], 0,
        "still inside its retention"
    );

    let (status, restored) = call(
        &s.app,
        "POST",
        "/api/db/rig/rejects/removed/restore",
        json!({"batch_id": batch}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{restored}");
    assert_eq!(restored["data"]["restored"].as_array().unwrap().len(), 1);
    assert!(s
        .root
        .join("images/M42/2026-10-01/LIGHT/M42_Ha_001.fits")
        .is_file());

    let (status, _) = call(
        &s.app,
        "POST",
        "/api/db/rig/rejects/removed/restore",
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn removal_needs_database_management() {
    let s = server();
    s.state.set_allow_database_management(false);
    for (method, uri, body) in [
        ("POST", "/api/db/rig/rejects/removal/preview", json!({})),
        (
            "POST",
            "/api/db/rig/rejects/removal/apply",
            json!({"min_age_days": 0, "digest": "x"}),
        ),
        ("GET", "/api/db/rig/rejects/removed", Value::Null),
        (
            "POST",
            "/api/db/rig/rejects/removed/restore",
            json!({"batch_id": "b"}),
        ),
        ("POST", "/api/db/rig/rejects/trash/empty", Value::Null),
    ] {
        let (status, body) = call(&s.app, method, uri, body).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{uri}: {body}");
    }
}
