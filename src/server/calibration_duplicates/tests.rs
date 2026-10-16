use super::*;
use rusqlite::params;
use std::path::{Path, PathBuf};
use std::time::Instant;

const NAME: &str = "2026-10-09_20-01-02_dark_0001.fits";

/// A catalog file in `dir` that records one dark twice: in `library` and
/// in `drop`, both on disk, and a server open on it.
struct Fixture {
    _dir: tempfile::TempDir,
    images: PathBuf,
    state: Arc<AppState>,
    context: Arc<DatabaseContext>,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let images = dir.path().join("images");
        let catalog = dir.path().join("catalog.sqlite");
        let conn = Connection::open(&catalog).unwrap();
        crate::calibration::ensure_schema(&conn).unwrap();
        for (id, uuid, folder) in [(1, "kept", "library"), (2, "copy", "drop")] {
            let path = images.join(folder).join(NAME);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"one dark exposure").unwrap();
            conn.execute(
                "INSERT INTO psf_guard_calibration_frame
                    (id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                     captured_at, file_size, added_at, updated_at)
                 VALUES (?1, ?2, 'rig', 'dark', ?3, ?2, 1000, 17, 0, 0)",
                params![id, uuid, path.to_string_lossy()],
            )
            .unwrap();
        }
        let state = Arc::new(AppState::new_for_test(conn));
        let mut context = (*state.all_databases().remove(0)).clone();
        context.image_dir_paths = vec![images.clone()];
        context.database_path = catalog.to_string_lossy().into_owned();
        Self {
            _dir: dir,
            images,
            state,
            context: Arc::new(context),
        }
    }

    fn ctx(&self) -> DbContext {
        DbContext(Arc::clone(&self.context))
    }

    fn path(&self) -> &str {
        &self.context.database_path
    }

    fn rows(&self) -> i64 {
        Connection::open(self.path())
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM psf_guard_calibration_frame",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    async fn check(&self) -> DuplicateJobStatus {
        let (code, Json(started)) = check_duplicates(State(Arc::clone(&self.state)), self.ctx())
            .await
            .unwrap();
        assert_eq!(code, StatusCode::ACCEPTED);
        started.data.unwrap()
    }

    async fn repair(&self, token: &str) -> Result<DuplicateJobStatus, AppError> {
        let (code, Json(started)) = repair_duplicates(
            State(Arc::clone(&self.state)),
            self.ctx(),
            Json(RepairRequest {
                token: token.into(),
            }),
        )
        .await?;
        assert_eq!(code, StatusCode::ACCEPTED);
        Ok(started.data.unwrap())
    }

    /// The status once the run `run` has ended.
    async fn finished(&self, run: u64) -> DuplicateJobStatus {
        let began = Instant::now();
        loop {
            let Json(now) = get_duplicates(self.ctx()).await;
            let now = now.data.unwrap();
            if now.run == run && now.state != "running" {
                return now;
            }
            assert!(
                began.elapsed() < Duration::from_secs(20),
                "run {run} did not end: {now:?}"
            );
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }
}

#[tokio::test]
async fn a_check_runs_as_a_job_and_the_status_route_never_does_the_work() {
    let fixture = Fixture::new();
    let Json(before) = get_duplicates(fixture.ctx()).await;
    let before = before.data.unwrap();
    assert_eq!(before.state, "idle");
    assert!(before.report.is_none(), "the status route plans nothing");

    let started = fixture.check().await;
    assert_eq!(
        (started.state.as_str(), started.action.as_deref()),
        ("running", Some("check"))
    );
    let done = fixture.finished(started.run).await;
    assert_eq!(done.state, "done", "{done:?}");
    let report = done.report.unwrap();
    assert_eq!((report.groups.len(), report.physical_rows), (1, 1));
    assert!(done.checked_at.is_some());
    // Both files were looked at, and the two copies compared.
    assert_eq!((done.done, done.total), (2, 2));
    assert!(
        !fixture.state.interactive_job_active(),
        "the person's guard is let go"
    );
}

#[tokio::test]
async fn a_repair_needs_management_and_the_previewed_plan() {
    let fixture = Fixture::new();
    let checked = fixture.check().await;
    let report = fixture.finished(checked.run).await.report.unwrap();

    assert!(matches!(
        fixture.repair(&report.token).await,
        Err(AppError::Forbidden(_))
    ));
    fixture.state.set_allow_database_management(true);

    // A token from another plan applies nothing, and says what changed.
    let stale = fixture.repair("an older preview").await.unwrap();
    let stale = fixture.finished(stale.run).await;
    assert_eq!(stale.state, "done");
    assert!(stale.repaired.is_none());
    let change = stale.changed.unwrap();
    assert_eq!((change.groups_before, change.groups_after), (1, 1));
    assert_eq!(fixture.rows(), 2, "nothing applied");
    assert_eq!(stale.report.unwrap().token, report.token, "the new plan");

    let applied = fixture.repair(&report.token).await.unwrap();
    let applied = fixture.finished(applied.run).await;
    assert_eq!(applied.state, "done", "{applied:?}");
    let repaired = applied.repaired.unwrap();
    assert_eq!(repaired.run, applied.run);
    assert_eq!(repaired.outcome.rows_removed, 1);
    assert!(applied.report.is_none(), "the plan is spent");
    assert!(applied.changed.is_none());
    assert_eq!((applied.stage, applied.done, applied.total), (None, 1, 1));
    assert_eq!(fixture.rows(), 1);
    // The merged rows' rig, named by the plan, is queued for flats.
    assert_eq!(
        crate::server::flat_measure::status(fixture.path()).rigs,
        ["rig"]
    );
    crate::server::flat_measure::cancel(fixture.path());
    assert!(fixture.images.join("drop").join(NAME).is_file());

    // The plan the token named is gone: a repair with it changes nothing.
    let again = fixture.repair(&report.token).await.unwrap();
    let again = fixture.finished(again.run).await;
    let change = again.changed.unwrap();
    assert_eq!((change.groups_before, change.groups_after), (0, 0));
    assert!(again.report.unwrap().groups.is_empty());
}

#[tokio::test]
async fn a_changed_plan_says_what_changed() {
    let fixture = Fixture::new();
    fixture.state.set_allow_database_management(true);
    let checked = fixture.check().await;
    let report = fixture.finished(checked.run).await.report.unwrap();
    // The second copy goes before Apply: the row merges as a missing one.
    std::fs::remove_file(fixture.images.join("drop").join(NAME)).unwrap();
    let started = fixture.repair(&report.token).await.unwrap();
    let ended = fixture.finished(started.run).await;
    assert_eq!(fixture.rows(), 2, "nothing applied");
    let change = ended.changed.unwrap();
    assert_eq!(change.altered, vec![NAME.to_string()]);
    assert_eq!((change.added_count, change.dropped_count), (0, 0));
    let fresh = ended.report.unwrap();
    assert_ne!(fresh.token, report.token);
    assert_eq!(fresh.missing_rows, 1);

    // The new plan applies.
    let applied = fixture.repair(&fresh.token).await.unwrap();
    let applied = fixture.finished(applied.run).await;
    assert_eq!(applied.repaired.unwrap().outcome.rows_removed, 1);
    crate::server::flat_measure::cancel(fixture.path());
}

#[tokio::test]
async fn stop_ends_a_check_and_one_run_at_a_time() {
    let fixture = Fixture::new();
    fixture.state.set_allow_database_management(true);
    let job = job(fixture.path());
    job.hold.store(true, Ordering::SeqCst);
    let started = fixture.check().await;
    // A second check joins the first; a repair waits its turn.
    assert_eq!(fixture.check().await.run, started.run);
    assert!(matches!(
        fixture.repair("token").await,
        Err(AppError::Conflict(_))
    ));
    let Json(stopping) = stop_duplicates(fixture.ctx()).await;
    assert_eq!(stopping.data.unwrap().run, started.run);
    let stopped = fixture.finished(started.run).await;
    assert_eq!(stopped.state, "stopped");
    assert!(stopped.report.is_none());
    job.hold.store(false, Ordering::SeqCst);

    // The next check starts afresh.
    let next = fixture.check().await;
    assert_eq!(next.run, started.run + 1);
    assert_eq!(fixture.finished(next.run).await.state, "done");
}

#[tokio::test]
async fn a_stopped_repair_rolls_back() {
    let fixture = Fixture::new();
    fixture.state.set_allow_database_management(true);
    let checked = fixture.check().await;
    let report = fixture.finished(checked.run).await.report.unwrap();
    let job = job(fixture.path());
    job.hold.store(true, Ordering::SeqCst);
    let started = fixture.repair(&report.token).await.unwrap();
    // Held at its first step; Stop takes effect before the first merge.
    stop(fixture.path());
    let stopped = fixture.finished(started.run).await;
    job.hold.store(false, Ordering::SeqCst);
    assert_eq!(stopped.state, "stopped");
    assert!(stopped.repaired.is_none());
    assert_eq!(fixture.rows(), 2);
}

#[tokio::test]
async fn a_repair_waits_for_no_stack_build_but_not_for_another_repair() {
    let fixture = Fixture::new();
    fixture.state.set_allow_database_management(true);
    let checked = fixture.check().await;
    let report = fixture.finished(checked.run).await.report.unwrap();
    // A stack build holds the worker for minutes on a large catalog; the
    // repair does not wait for it.
    let _build = fixture
        .state
        .stack_previews
        .acquire_maintenance_permit()
        .await
        .unwrap();
    // The command line is repairing the catalog: this one is refused.
    let cli = crate::calibration::repair_lock(Path::new(fixture.path())).unwrap();
    let refused = fixture.repair(&report.token).await.unwrap();
    let refused = fixture.finished(refused.run).await;
    assert_eq!(refused.state, "failed");
    assert!(
        refused.error.unwrap().contains("Another repair"),
        "names the other repair"
    );
    assert_eq!(fixture.rows(), 2);
    drop(cli);

    let applied = fixture.repair(&report.token).await.unwrap();
    let applied = tokio::time::timeout(Duration::from_secs(10), fixture.finished(applied.run))
        .await
        .expect("the repair runs while a stack build holds the worker");
    assert_eq!(applied.repaired.unwrap().outcome.rows_removed, 1);
    crate::server::flat_measure::cancel(fixture.path());
}

#[test]
fn plan_change_names_what_was_added_dropped_and_altered() {
    let group = |keep: &str, name: &str, exists: bool| {
        let mut group = TwinGroup::for_test();
        group.file_name = name.into();
        group.rig_uuid = "rig".into();
        group.keep = keep.into();
        group.path = format!("/{name}");
        group.physical = exists;
        group.members = vec![
            crate::calibration::TwinMember {
                frame_uuid: keep.into(),
                source_path: format!("/{name}"),
                exists: true,
                keep: true,
            },
            crate::calibration::TwinMember {
                frame_uuid: format!("{keep}-copy"),
                source_path: format!("/copy/{name}"),
                exists,
                keep: false,
            },
        ];
        group
    };
    let report = |groups: Vec<TwinGroup>| {
        let mut report = DuplicateReport::for_test();
        report.rows_removed = groups.len();
        report.groups = groups;
        report
    };
    let before = report(vec![
        group("a", "a.fits", true),
        group("b", "b.fits", true),
        group("c", "c.fits", true),
    ]);
    let after = report(vec![
        group("a", "a.fits", true),
        group("b", "b.fits", false),
        group("d", "d.fits", true),
    ]);
    let change = plan_change(Some(&before), &after);
    assert_eq!(change.added, ["d.fits"]);
    assert_eq!(change.dropped, ["c.fits"]);
    assert_eq!(change.altered, ["b.fits"]);
    assert_eq!((change.groups_before, change.groups_after), (3, 3));
}
