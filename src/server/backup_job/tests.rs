use super::*;
use crate::auth_registry::{AccessRole, AuthRegistry, AuthTokenRecord, AuthUserRecord};
use axum::body::Body;
use axum::http::Request;
use rusqlite::Connection;
use tower::ServiceExt;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    state: Arc<AppState>,
    registry: PathBuf,
}

/// A registry with one catalog of `frames` rows of 2 KiB, on a server that
/// allows database management.
fn fixture(frames: usize) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let catalog = root.join("rig").join("schedulerdb.sqlite");
    std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
    let connection = Connection::open(&catalog).unwrap();
    connection
        .execute_batch(
            "PRAGMA user_version = 21;
             CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, payload BLOB);",
        )
        .unwrap();
    let mut insert = connection
        .prepare("INSERT INTO acquiredimage (payload) VALUES (?1)")
        .unwrap();
    for _ in 0..frames {
        insert.execute([vec![3u8; 2048]]).unwrap();
    }
    drop(insert);
    drop(connection);
    let registry = root.join("config").join("config.json");
    let mut entries = DbRegistry::default();
    entries
        .add(
            "Imaging Rig".into(),
            catalog.display().to_string(),
            vec![],
            Some("imaging-rig".into()),
        )
        .unwrap();
    entries.save(&registry).unwrap();
    let state = AppState::from_databases(
        vec![],
        crate::server::storage::StorageRoots::single(root.join("cache")),
        crate::cli::PregenerationConfig::default(),
    )
    .unwrap();
    state.set_registry_path(Some(registry.clone()));
    state.set_allow_database_management(true);
    Fixture {
        _dir: dir,
        root,
        state: Arc::new(state),
        registry,
    }
}

fn wait_for_the_run(state: &AppState) -> LastRun {
    let began = std::time::Instant::now();
    while state.backups.job().is_some() {
        assert!(
            began.elapsed() < Duration::from_secs(60),
            "the run never ended"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    state.backups.last_run().expect("a recorded run")
}

fn files_in(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn manual(scope: BackupScope, destination: Option<PathBuf>) -> RunRequest {
    RunRequest {
        trigger: Trigger::Manual,
        scope,
        include_secrets: false,
        destination,
    }
}

#[test]
fn a_run_without_a_folder_makes_an_archive_to_download() {
    let fixture = fixture(20);
    start(&fixture.state, manual(BackupScope::Full, None)).unwrap();
    let last = wait_for_the_run(&fixture.state);
    assert_eq!(last.outcome, Outcome::Succeeded, "{:?}", last.error);
    assert_eq!(last.location, Location::Downloads);
    let name = last.file_name.unwrap();
    let downloads = fixture.root.join("cache").join(DOWNLOADS_FOLDER);
    assert_eq!(files_in(&downloads), vec![name.clone()]);
    let view = overview(&fixture.state, &fixture.registry).unwrap();
    assert_eq!(view.backups.len(), 1);
    assert_eq!(view.backups[0].location, Location::Downloads);
    assert_eq!(
        view.backups[0].download_path,
        format!("/backups/files/downloads/{name}")
    );
    assert_eq!(view.catalogs, 1);
    assert!(view.catalog_bytes > 40_000);
}

#[test]
fn a_second_run_is_refused_while_one_runs() {
    let fixture = fixture(4000);
    fixture
        .state
        .backups
        .set_step_pause(Duration::from_millis(30));
    let folder = fixture.root.join("backups");
    start(
        &fixture.state,
        manual(BackupScope::Full, Some(folder.clone())),
    )
    .unwrap();
    assert!(matches!(
        start(&fixture.state, manual(BackupScope::Meta, Some(folder))),
        Err(StartError::Busy)
    ));
    fixture.state.backups.stop();
    wait_for_the_run(&fixture.state);
}

#[test]
fn stop_ends_the_run_and_leaves_nothing_behind() {
    let fixture = fixture(4000);
    fixture
        .state
        .backups
        .set_step_pause(Duration::from_millis(30));
    let folder = fixture.root.join("backups");
    start(
        &fixture.state,
        manual(BackupScope::Full, Some(folder.clone())),
    )
    .unwrap();
    // Into the catalog's copy.
    let began = std::time::Instant::now();
    while fixture
        .state
        .backups
        .job()
        .is_some_and(|job| job.current.as_deref() != Some("catalog imaging-rig"))
    {
        assert!(began.elapsed() < Duration::from_secs(30));
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(fixture.state.backups.stop());
    assert!(fixture.state.backups.job().unwrap().stopping);
    let last = wait_for_the_run(&fixture.state);
    assert_eq!(last.outcome, Outcome::Stopped);
    assert!(files_in(&folder).is_empty(), "{:?}", files_in(&folder));
    // A new run starts afterwards.
    start(&fixture.state, manual(BackupScope::Meta, Some(folder))).unwrap();
    assert_eq!(wait_for_the_run(&fixture.state).outcome, Outcome::Succeeded);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_servers_stop_ends_a_run_and_waits_for_it() {
    let fixture = fixture(4000);
    fixture
        .state
        .backups
        .set_step_pause(Duration::from_millis(30));
    let folder = fixture.root.join("backups");
    start(
        &fixture.state,
        manual(BackupScope::Full, Some(folder.clone())),
    )
    .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    fixture.state.shutdown.begin();
    tokio::time::timeout(Duration::from_secs(30), fixture.state.shutdown.drained())
        .await
        .expect("the stop waited too long");
    // The runner records how it ended just after it lets go of the stop.
    let last = wait_for_the_run(&fixture.state);
    assert_eq!(last.outcome, Outcome::Stopped);
    assert!(files_in(&folder).is_empty());
    // A stopping server starts no scheduled run.
    assert!(!tick(&fixture.state, Utc::now()));
}

fn schedule(fixture: &Fixture, settings: BackupSettings) {
    let mut registry = DbRegistry::load_or_init(&fixture.registry).unwrap();
    registry.backups = Some(settings);
    registry.save(&fixture.registry).unwrap();
}

#[test]
fn a_due_schedule_runs_once_then_waits_for_its_next_slot() {
    let fixture = fixture(10);
    let folder = fixture.root.join("scheduled");
    let now = Utc::now();
    schedule(
        &fixture,
        BackupSettings {
            enabled: true,
            // Midnight local has passed today, and the schedule is older.
            time: "00:00".into(),
            destination: Some(folder.display().to_string()),
            since: Some(now.timestamp() - 3 * 86_400),
            retention: backup::schedule::Retention::Count { count: 1 },
            ..Default::default()
        },
    );
    assert!(tick(&fixture.state, now), "a missed slot runs");
    let last = wait_for_the_run(&fixture.state);
    assert_eq!(last.outcome, Outcome::Succeeded, "{:?}", last.error);
    assert_eq!(last.trigger, Trigger::Schedule);
    assert_eq!(last.location, Location::Destination);
    assert!(last.file_name.unwrap().ends_with("-full-auto.zip"));
    // Done for this slot: the next is tomorrow.
    assert!(!tick(&fixture.state, now));
    let settings = DbRegistry::load_or_init(&fixture.registry)
        .unwrap()
        .backups
        .unwrap();
    let next = fixture.state.backups.next_run_at(&settings, now).unwrap();
    assert!(next > now.timestamp());
    assert!(next <= now.timestamp() + 2 * 86_400);
    // The listing finds it in the schedule's folder.
    let view = overview(&fixture.state, &fixture.registry).unwrap();
    assert_eq!(view.backups.len(), 1);
    assert_eq!(view.backups[0].location, Location::Destination);
}

#[test]
fn a_failed_scheduled_run_waits_an_hour_before_the_next_try() {
    let fixture = fixture(10);
    // A file where the folder should be: the run cannot write there.
    let blocked = fixture.root.join("not-a-folder");
    std::fs::write(&blocked, "file").unwrap();
    let now = Utc::now();
    let settings = BackupSettings {
        enabled: true,
        time: "00:00".into(),
        destination: Some(blocked.display().to_string()),
        since: Some(now.timestamp() - 3 * 86_400),
        ..Default::default()
    };
    schedule(&fixture, settings.clone());
    assert!(tick(&fixture.state, now));
    let last = wait_for_the_run(&fixture.state);
    assert_eq!(last.outcome, Outcome::Failed);
    assert!(last.error.is_some());
    let next = fixture.state.backups.next_run_at(&settings, now).unwrap();
    assert!(next >= now.timestamp() + 3000, "retries after an hour");
    assert!(!tick(&fixture.state, Utc::now()));
}

#[test]
fn a_stopped_scheduled_run_waits_for_the_next_slot() {
    let fixture = fixture(4000);
    fixture
        .state
        .backups
        .set_step_pause(Duration::from_millis(30));
    let now = Utc::now();
    let settings = BackupSettings {
        enabled: true,
        time: "00:00".into(),
        destination: Some(fixture.root.join("s").display().to_string()),
        since: Some(now.timestamp() - 3 * 86_400),
        ..Default::default()
    };
    schedule(&fixture, settings.clone());
    assert!(tick(&fixture.state, now));
    fixture.state.backups.stop();
    assert_eq!(wait_for_the_run(&fixture.state).outcome, Outcome::Stopped);
    assert!(
        !tick(&fixture.state, Utc::now()),
        "a stop is not undone at once"
    );
    assert!(
        fixture
            .state
            .backups
            .next_run_at(&settings, Utc::now())
            .unwrap()
            > now.timestamp()
    );
}

async fn status_of(
    state: &Arc<AppState>,
    method: &str,
    path: &str,
    token: Option<&str>,
) -> StatusCode {
    let mut request = Request::builder().method(method).uri(path);
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let body = if method == "GET" {
        Body::empty()
    } else {
        request = request.header("content-type", "application/json");
        Body::from("{}")
    };
    crate::server::api_router(Arc::clone(state))
        .oneshot(request.body(body).unwrap())
        .await
        .unwrap()
        .status()
}

#[tokio::test]
async fn backups_need_the_management_gate_and_an_editor() {
    let fixture = fixture(1);
    let state = &fixture.state;
    assert_eq!(
        status_of(state, "GET", "/backups", None).await,
        StatusCode::OK
    );
    assert_eq!(
        status_of(state, "GET", "/backups/status", None).await,
        StatusCode::OK
    );

    state.set_allow_database_management(false);
    for (method, path) in [
        ("GET", "/backups"),
        ("GET", "/backups/status"),
        ("POST", "/backups/run"),
        ("POST", "/backups/stop"),
        ("PUT", "/backups/settings"),
        ("POST", "/backups/check-destination"),
        (
            "GET",
            "/backups/files/downloads/psf-guard-backup-2026-10-16T130005Z-meta.zip",
        ),
    ] {
        assert_eq!(
            status_of(state, method, path, None).await,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
    assert!(state.backups.job().is_none(), "nothing ran");

    // With accounts, a viewer cannot even list them; an editor can.
    state.set_allow_database_management(true);
    let mut accounts = AuthRegistry::default();
    accounts
        .add(
            AuthUserRecord::new("viewer", AccessRole::ReadOnly, "viewer-password").unwrap(),
            false,
        )
        .unwrap();
    accounts
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-password").unwrap(),
            false,
        )
        .unwrap();
    let (viewer, record) = AuthTokenRecord::mint("viewer", "viewer token", true, None).unwrap();
    accounts.tokens.push(record);
    let (editor, record) = AuthTokenRecord::mint("editor", "editor token", false, None).unwrap();
    accounts.tokens.push(record);
    state.set_server_auth(
        crate::server::auth::ServerAuth::from_sources(None, &accounts, 3000).unwrap(),
    );
    assert_eq!(
        status_of(state, "GET", "/backups", Some(&viewer)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(state, "GET", "/backups/status", Some(&viewer)).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(state, "GET", "/backups", Some(&editor)).await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn a_download_names_only_an_archive_in_a_known_folder() {
    let fixture = fixture(1);
    let state = &fixture.state;
    assert_eq!(
        status_of(
            state,
            "GET",
            "/backups/files/downloads/..%2Fconfig.json",
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        status_of(
            state,
            "GET",
            "/backups/files/elsewhere/psf-guard-backup-2026-10-16T130005Z-meta.zip",
            None
        )
        .await,
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn saving_a_schedule_validates_and_stamps_it() {
    let fixture = fixture(1);
    let app = crate::server::api_router(Arc::clone(&fixture.state));
    let put = |body: serde_json::Value| {
        Request::builder()
            .method("PUT")
            .uri("/backups/settings")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let refused = app
        .clone()
        .oneshot(put(serde_json::json!({"enabled": true})))
        .await
        .unwrap();
    assert_eq!(refused.status(), StatusCode::BAD_REQUEST);
    let folder = fixture.root.join("b").display().to_string();
    let saved = app
        .oneshot(put(serde_json::json!({
            "enabled": true, "destination": folder, "time": "12:30",
            "retention": {"kind": "age", "days": 14}
        })))
        .await
        .unwrap();
    assert_eq!(saved.status(), StatusCode::OK);
    let settings = DbRegistry::load_or_init(&fixture.registry)
        .unwrap()
        .backups
        .unwrap();
    assert!(settings.enabled);
    assert_eq!(settings.time, "12:30");
    assert!(settings.since.is_some());
    assert_eq!(
        settings.retention,
        backup::schedule::Retention::Age { days: 14 }
    );
}
