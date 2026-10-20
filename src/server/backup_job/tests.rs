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
    // The run's end shows in memory before `remember` writes it to disk, and
    // `remember` holds `saving` until the file is written. Taking the lock
    // waits for that write, so a restarted manager reads the finished run.
    drop(
        state
            .backups
            .saving
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    );
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
    fixture
        .state
        .backups
        .stop(&fixture.state.catalog_jobs.backups);
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
    assert!(fixture
        .state
        .backups
        .stop(&fixture.state.catalog_jobs.backups));
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

/// Save a schedule as Settings does: the folder made, an install id kept.
fn schedule(fixture: &Fixture, mut settings: BackupSettings) {
    if let Some(folder) = settings.destination_path().map(PathBuf::from)
        && !folder.exists()
    {
        std::fs::create_dir(&folder).unwrap();
    }
    settings.install_id = Some("test-install".into());
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
    let next = fixture
        .state
        .backups
        .next_run_at(&settings, "test-install", now)
        .unwrap();
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
    let next = fixture
        .state
        .backups
        .next_run_at(&settings, "test-install", now)
        .unwrap();
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
    fixture
        .state
        .backups
        .stop(&fixture.state.catalog_jobs.backups);
    assert_eq!(wait_for_the_run(&fixture.state).outcome, Outcome::Stopped);
    assert!(
        !tick(&fixture.state, Utc::now()),
        "a stop is not undone at once"
    );
    assert!(
        fixture
            .state
            .backups
            .next_run_at(&settings, "test-install", Utc::now())
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

/// A scheduled run that leaves a catalog out (unreachable, or "kept
/// changing") must not prune: after `count` such days the last archive
/// holding that catalog would be gone.
#[test]
fn a_partial_scheduled_backup_keeps_the_last_complete_one() {
    let fixture = fixture(10);
    let folder = fixture.root.join("scheduled");
    let now = Utc::now();
    schedule(
        &fixture,
        BackupSettings {
            enabled: true,
            time: "00:00".into(),
            destination: Some(folder.display().to_string()),
            since: Some(now.timestamp() - 3 * 86_400),
            retention: backup::schedule::Retention::Count { count: 1 },
            ..Default::default()
        },
    );
    assert!(tick(&fixture.state, now));
    let complete = wait_for_the_run(&fixture.state).file_name.unwrap();
    // The NAS holding the catalog drops off before the next day's run.
    let catalog = fixture.root.join("rig").join("schedulerdb.sqlite");
    std::fs::rename(&catalog, fixture.root.join("rig").join("away.sqlite")).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    assert!(tick(&fixture.state, now + chrono::Duration::hours(25)));
    let partial = wait_for_the_run(&fixture.state);
    assert_eq!(partial.outcome, Outcome::Succeeded);
    assert!(!partial.problems.is_empty() || !partial.pruned.is_empty());
    assert!(
        folder.join(&complete).exists(),
        "pruned the only archive with the catalog: {:?}",
        partial.pruned
    );
}

/// Stop ends a scheduled run that is waiting for a person's work, without
/// waiting for that work to end.
#[test]
fn stop_ends_a_scheduled_run_that_waits_for_interactive_work() {
    let fixture = fixture(10);
    let now = Utc::now();
    schedule(
        &fixture,
        BackupSettings {
            enabled: true,
            time: "00:00".into(),
            destination: Some(fixture.root.join("s").display().to_string()),
            since: Some(now.timestamp() - 3 * 86_400),
            ..Default::default()
        },
    );
    let stack_build = fixture.state.begin_named_interactive_job("Stack build");
    assert!(tick(&fixture.state, now));
    std::thread::sleep(Duration::from_millis(300));
    assert!(fixture
        .state
        .backups
        .stop(&fixture.state.catalog_jobs.backups));
    let began = std::time::Instant::now();
    while fixture.state.backups.job().is_some() && began.elapsed() < Duration::from_secs(3) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let still_running = fixture.state.backups.job().is_some();
    drop(stack_build);
    wait_for_the_run(&fixture.state);
    assert!(!still_running, "Stop waited for the person's work to end");
}

/// Archives in the download area go after a day even when no other
/// download run comes along: the schedule's tick sweeps them.
#[test]
fn a_download_older_than_a_day_goes_without_another_download() {
    let fixture = fixture(1);
    let registry = DbRegistry::load_or_init(&fixture.registry).unwrap();
    let plan = backup::plan(
        &locations(&fixture.state, fixture.registry.clone()),
        &registry,
        BackupScope::Meta,
        true,
    );
    std::fs::create_dir_all(downloads_folder(&fixture.state)).unwrap();
    let old = backup::write(
        &plan,
        &downloads_folder(&fixture.state),
        Trigger::Manual,
        Utc::now() - chrono::Duration::days(3),
        &backup::Hooks::quiet(),
    )
    .unwrap();
    // The server keeps running: the schedule ticks, Settings looks.
    tick(&fixture.state, Utc::now());
    overview(&fixture.state, &fixture.registry).unwrap();
    assert!(!old.path.exists(), "a three-day-old download is still kept");
}

/// A read-write API token, which may not even list API tokens, must not
/// take the peer keys, collaboration tokens and server TOML tokens out in
/// the clear through "include secrets".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_api_token_cannot_take_secrets_out() {
    let fixture = fixture(1);
    let state = &fixture.state;
    let mut accounts = AuthRegistry::default();
    accounts
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-password").unwrap(),
            false,
        )
        .unwrap();
    let (token, record) = AuthTokenRecord::mint("editor", "script", false, None).unwrap();
    accounts.tokens.push(record);
    state.set_server_auth(
        crate::server::auth::ServerAuth::from_sources(None, &accounts, 3000).unwrap(),
    );
    let response = crate::server::api_router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/backups/run")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scope":"meta","include_secrets":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    if state.backups.job().is_some() {
        tokio::task::block_in_place(|| wait_for_the_run(state));
    }
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn secrets_are_never_offered_as_a_download() {
    let fixture = fixture(1);
    let response = crate::server::api_router(Arc::clone(&fixture.state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/backups/run")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"scope":"meta","include_secrets":true}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(fixture.state.backups.job().is_none());
    // To a server folder, from the desktop app or a signed-in person: yes.
    let folder = fixture.root.join("private");
    let body = serde_json::json!({
        "scope": "meta", "include_secrets": true, "destination": folder.display().to_string(),
    });
    let response = crate::server::api_router(Arc::clone(&fixture.state))
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/backups/run")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::ACCEPTED);
    tokio::task::spawn_blocking({
        let state = Arc::clone(&fixture.state);
        move || wait_for_the_run(&state)
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_api_token_cannot_download_an_archive_with_secrets_or_schedule_one() {
    let fixture = fixture(1);
    let state = &fixture.state;
    let folder = fixture.root.join("scheduled");
    schedule(
        &fixture,
        BackupSettings {
            destination: Some(folder.display().to_string()),
            ..Default::default()
        },
    );
    let registry = DbRegistry::load_or_init(&fixture.registry).unwrap();
    let locations = locations(state, fixture.registry.clone());
    let with_secrets = backup::write(
        &backup::plan(&locations, &registry, BackupScope::Meta, true),
        &folder,
        Trigger::Manual,
        Utc::now(),
        &backup::Hooks::quiet(),
    )
    .unwrap();
    let without = backup::write(
        &backup::plan(&locations, &registry, BackupScope::Meta, false),
        &folder,
        Trigger::Manual,
        Utc::now() + chrono::Duration::seconds(1),
        &backup::Hooks::quiet(),
    )
    .unwrap();
    let mut accounts = AuthRegistry::default();
    accounts
        .add(
            AuthUserRecord::new("editor", AccessRole::ReadWrite, "editor-password").unwrap(),
            false,
        )
        .unwrap();
    let (token, record) = AuthTokenRecord::mint("editor", "script", false, None).unwrap();
    accounts.tokens.push(record);
    state.set_server_auth(
        crate::server::auth::ServerAuth::from_sources(None, &accounts, 3000).unwrap(),
    );
    assert_eq!(
        status_of(
            state,
            "GET",
            &format!("/backups/files/destination/{}", with_secrets.file_name),
            Some(&token)
        )
        .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        status_of(
            state,
            "GET",
            &format!("/backups/files/destination/{}", without.file_name),
            Some(&token)
        )
        .await,
        StatusCode::OK
    );
    let response = crate::server::api_router(Arc::clone(state))
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/backups/settings")
                .header("authorization", format!("Bearer {token}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({
                        "destination": folder.display().to_string(), "include_secrets": true,
                    })
                    .to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
}

#[test]
fn a_stopped_or_failed_run_is_remembered_across_a_restart() {
    let fixture = fixture(4000);
    fixture.state.backups.load_state(&fixture.registry);
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
    fixture
        .state
        .backups
        .stop(&fixture.state.catalog_jobs.backups);
    assert_eq!(wait_for_the_run(&fixture.state).outcome, Outcome::Stopped);
    assert!(state_path_for(&fixture.registry).is_file());
    // A new server reads the same registry folder.
    let restarted = BackupManager::default();
    restarted.load_state(&fixture.registry);
    let next = restarted
        .next_run_at(&settings, "test-install", Utc::now())
        .unwrap();
    assert!(
        next > Utc::now().timestamp(),
        "the stopped run would start again at once"
    );
    // Settings still says how the last backup went.
    let last = restarted
        .last_run()
        .expect("the last run survives a restart");
    assert_eq!(last.outcome, Outcome::Stopped);
    assert_eq!(last.trigger, Trigger::Schedule);
}

#[test]
fn remembered_state_is_not_visible_before_its_file_is_saved() {
    let fixture = fixture(1);
    let manager = Arc::new(BackupManager::default());
    manager.load_state(&fixture.registry);
    // Hold the writer just before it saves, after it changes the state.
    let path = manager.state_path.write().unwrap();
    let (changed, changed_rx) = std::sync::mpsc::channel();
    let writer = Arc::clone(&manager);
    let writing = std::thread::spawn(move || {
        writer.remember(|inner| {
            inner.held_since = Some(1234);
            changed.send(()).unwrap();
        });
    });
    changed_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (visible, visible_rx) = std::sync::mpsc::channel();
    let reader = Arc::clone(&manager);
    let reading = std::thread::spawn(move || {
        let held = reader.inner.lock().unwrap().held_since;
        visible.send(held).unwrap();
    });
    let early = visible_rx.recv_timeout(Duration::from_millis(100));
    // Always release and join the workers before asserting, even on failure.
    drop(path);
    writing.join().unwrap();
    reading.join().unwrap();
    assert!(
        matches!(early, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
        "the new state became visible before its file was saved: {early:?}"
    );
    assert_eq!(visible_rx.recv().unwrap(), Some(1234));
    let restarted = BackupManager::default();
    restarted.load_state(&fixture.registry);
    assert_eq!(restarted.inner.lock().unwrap().held_since, Some(1234));
}

#[test]
fn the_last_backup_is_remembered_across_a_restart() {
    let fixture = fixture(1);
    fixture.state.backups.load_state(&fixture.registry);
    let folder = fixture.root.join("now");
    start(&fixture.state, manual(BackupScope::Meta, Some(folder))).unwrap();
    let finished = wait_for_the_run(&fixture.state);
    assert_eq!(finished.outcome, Outcome::Succeeded);
    let restarted = BackupManager::default();
    assert!(restarted.last_run().is_none());
    restarted.load_state(&fixture.registry);
    assert_eq!(restarted.last_run(), Some(finished));
}

#[test]
fn saving_the_schedule_mints_one_install_id_and_makes_its_folder() {
    let fixture = fixture(1);
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let folder = fixture.root.join("made-by-save");
    let save = |body: serde_json::Value| {
        let app = crate::server::api_router(Arc::clone(&fixture.state));
        runtime.block_on(async move {
            app.oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/backups/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
            .status()
        })
    };
    let body = serde_json::json!({"enabled": true, "destination": folder.display().to_string()});
    assert_eq!(save(body.clone()), StatusCode::OK);
    assert!(folder.is_dir());
    let first = DbRegistry::load_or_init(&fixture.registry)
        .unwrap()
        .backups
        .unwrap()
        .install_id
        .unwrap();
    let mut forged = body;
    forged["install_id"] = serde_json::json!("someone-else");
    assert_eq!(save(forged), StatusCode::OK);
    let second = DbRegistry::load_or_init(&fixture.registry)
        .unwrap()
        .backups
        .unwrap()
        .install_id
        .unwrap();
    assert_eq!(first, second);
    // A folder on a disk that is not there is refused, and not made.
    let unmounted = fixture.root.join("Volumes").join("Backup").join("psf");
    let body = serde_json::json!({"enabled": true, "destination": unmounted.display().to_string()});
    assert_eq!(save(body), StatusCode::BAD_REQUEST);
    assert!(!fixture.root.join("Volumes").exists());
}

/// A catalog that stays unreachable must not stop pruning for good: later
/// runs that lack it prune each other, and only the archives holding it are
/// kept, which the last run says.
#[test]
fn an_unreachable_catalog_does_not_stop_pruning() {
    let fixture = fixture(10);
    let folder = fixture.root.join("scheduled");
    let now = Utc::now();
    schedule(
        &fixture,
        BackupSettings {
            enabled: true,
            time: "00:00".into(),
            destination: Some(folder.display().to_string()),
            since: Some(now.timestamp() - 3 * 86_400),
            retention: backup::schedule::Retention::Count { count: 1 },
            ..Default::default()
        },
    );
    assert!(tick(&fixture.state, now));
    let complete = wait_for_the_run(&fixture.state).file_name.unwrap();
    let catalog = fixture.root.join("rig").join("schedulerdb.sqlite");
    std::fs::rename(&catalog, fixture.root.join("rig").join("away.sqlite")).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    assert!(tick(&fixture.state, now + chrono::Duration::hours(25)));
    let second = wait_for_the_run(&fixture.state);
    assert_eq!(second.prune_kept, vec![complete.clone()]);
    assert_eq!(second.prune_kept_for, vec!["Imaging Rig".to_string()]);
    assert!(second.prune_skipped.is_none());
    std::thread::sleep(Duration::from_millis(1100));
    assert!(tick(&fixture.state, now + chrono::Duration::hours(50)));
    let third = wait_for_the_run(&fixture.state);
    assert_eq!(third.outcome, Outcome::Succeeded);
    assert_eq!(third.pruned, vec![second.file_name.clone().unwrap()]);
    assert_eq!(third.prune_kept, vec![complete.clone()]);
    assert!(folder.join(&complete).exists());
    assert!(folder.join(third.file_name.unwrap()).exists());
    assert_eq!(backup::list(&folder).len(), 2);
}

/// An archive with secrets, or one whose manifest cannot be read, is never
/// offered as a download: not to the desktop app, a signed-in person or a
/// token alike.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_one_downloads_an_archive_with_secrets_or_an_unreadable_manifest() {
    let fixture = fixture(1);
    let state = &fixture.state;
    let folder = fixture.root.join("scheduled");
    schedule(
        &fixture,
        BackupSettings {
            destination: Some(folder.display().to_string()),
            ..Default::default()
        },
    );
    let registry = DbRegistry::load_or_init(&fixture.registry).unwrap();
    let locations = locations(state, fixture.registry.clone());
    let with_secrets = backup::write(
        &backup::plan(&locations, &registry, BackupScope::Meta, true),
        &folder,
        Trigger::Manual,
        Utc::now(),
        &backup::Hooks::quiet(),
    )
    .unwrap();
    let without = backup::write(
        &backup::plan(&locations, &registry, BackupScope::Meta, false),
        &folder,
        Trigger::Manual,
        Utc::now() + chrono::Duration::seconds(1),
        &backup::Hooks::quiet(),
    )
    .unwrap();
    let unreadable = "psf-guard-backup-2026-10-16T130005Z-meta.zip";
    std::fs::write(folder.join(unreadable), "not a zip").unwrap();
    let get = |name: String| async move {
        status_of(
            state,
            "GET",
            &format!("/backups/files/destination/{name}"),
            None,
        )
        .await
    };
    assert_eq!(
        get(with_secrets.file_name.clone()).await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(get(unreadable.to_string()).await, StatusCode::FORBIDDEN);
    assert_eq!(get(without.file_name.clone()).await, StatusCode::OK);
    assert_eq!(
        get("psf-guard-backup-2026-10-16T130006Z-meta.zip".into()).await,
        StatusCode::NOT_FOUND
    );
}

#[test]
fn a_failed_runner_frees_the_slot_and_records_the_run_as_failed() {
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
    assert!(fixture.state.backups.job().is_some());
    // The runner thread is lost, as on a panic: the slot frees, and the run
    // is recorded as failed.
    let mut queue = Queue {
        server: Arc::downgrade(&fixture.state),
        ..Queue::default()
    };
    Backups::runner_failed(&mut queue, &mut ());
    assert!(fixture.state.backups.job().is_none());
    let last = fixture.state.backups.last_run().unwrap();
    assert_eq!(last.outcome, Outcome::Failed);
    assert_eq!(
        last.error.as_deref(),
        Some("the backup stopped unexpectedly")
    );
    fixture.state.shutdown.begin();
}
