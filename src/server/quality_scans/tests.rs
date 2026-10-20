use super::*;
use std::time::Duration;

/// A catalog file with two targets and no frames, and a server that lists
/// it. The runner reads the catalog on a connection of its own, so the
/// catalog must be a file.
fn catalog(directory: &std::path::Path) -> (Arc<AppState>, Arc<DatabaseContext>) {
    let path = directory.join("catalog.sqlite");
    let conn = rusqlite::Connection::open(&path).unwrap();
    crate::ts_schema::apply_schema(&conn).unwrap();
    conn.execute_batch(
        "INSERT INTO project(Id,name,profileId,guid) VALUES(1,'Project','profile','project-one');
         INSERT INTO target(Id,name,projectId,active,ra,dec,epochcode,rotation,roi,guid)
             VALUES(1,'One',1,1,10,20,0,0,100,'target-one'),
                   (2,'Two',1,1,11,21,0,0,100,'target-two');",
    )
    .unwrap();
    let mut ctx = DatabaseContext::new_for_test(conn);
    ctx.database_path = path.to_string_lossy().into_owned();
    ctx.use_storage_for_test(directory);
    let ctx = Arc::new(ctx);
    let state = Arc::new(AppState::new_for_test(
        rusqlite::Connection::open_in_memory().unwrap(),
    ));
    state
        .databases
        .write()
        .unwrap()
        .insert(ctx.id.clone(), Arc::clone(&ctx));
    (state, ctx)
}

fn request(target_id: i32) -> SpatialScanRequest {
    SpatialScanRequest {
        target_id,
        filter_name: None,
        force: false,
        force_spatial: false,
        force_astrometry: false,
        force_satellites: false,
        fill_metadata: Some(false),
    }
}

async fn until(what: &str, done: impl Fn() -> bool) {
    for _ in 0..500 {
        if done() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("{what}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_queued_scan_shows_as_preparing_and_finishes_when_nothing_needs_computing() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(directory.path());
    assert!(queue_scan(&state, &ctx, request(1)));
    // The route answers before the runner reads the catalog.
    let (progress, _) = spatial_scan::progress_snapshot(&ctx.spatial_metrics);
    assert!(progress.running);
    assert_eq!(progress.target_id, Some(1));
    let job = state.catalog_jobs.quality_scans.job(&scanner(&ctx));
    until("the runner never exited", || !job.queue().has_runner()).await;
    let (progress, _) = spatial_scan::progress_snapshot(&ctx.spatial_metrics);
    assert!(!progress.running, "no frame needed computing");
    assert_eq!(progress.stage, "complete");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_analysis_scans_each_target_then_finishes_and_refuses_a_second_meanwhile() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(directory.path());
    assert!(queue_analysis(&state, &ctx, vec![1, 2], false, false));
    assert!(
        !queue_analysis(&state, &ctx, vec![1], false, false),
        "an analysis is already queued"
    );
    let job = state.catalog_jobs.quality_scans.job(&scanner(&ctx));
    until("the analysis never finished", || {
        !job.queue().has_runner() && !quality_backfill::snapshot(&ctx.quality_backfill).running
    })
    .await;
    let progress = quality_backfill::snapshot(&ctx.quality_backfill);
    assert_eq!(progress.processed_targets, 2);
    assert!(progress.finished_at.is_some());
    assert!(
        queue_analysis(&state, &ctx, vec![1], false, false),
        "a new one may start"
    );
}

#[tokio::test]
async fn a_persons_scans_run_before_the_analysis_targets() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(directory.path());
    let runtime = tokio::runtime::Handle::current();
    let mut queue = Queue {
        analysis: Some(Analysis {
            ctx: Arc::clone(&ctx),
            targets: VecDeque::from([1, 2]),
            force: false,
            fill_metadata: false,
            runtime: runtime.clone(),
        }),
        ..Queue::default()
    };
    let now = Instant::now();
    let next = |queue: &mut Queue| QualityScans::next(&state, "c", queue, now);
    assert!(matches!(
        next(&mut queue),
        Step::Run(Item::Target { target_id: 1, .. })
    ));
    assert!(queue.analysis_running);
    queue.scans.push_back(PersonScan {
        ctx: Arc::clone(&ctx),
        request: request(7),
        runtime,
    });
    match next(&mut queue) {
        Step::Run(Item::Scan(scan)) => assert_eq!(scan.request.target_id, 7),
        _ => panic!("the person's scan runs next"),
    }
    assert!(!queue.analysis_running);
    assert!(matches!(
        next(&mut queue),
        Step::Run(Item::Target { target_id: 2, .. })
    ));
    assert!(matches!(next(&mut queue), Step::Run(Item::AnalysisDone(_))));
    assert!(queue.analysis.is_none());
    assert!(matches!(next(&mut queue), Step::Exit));
}

#[tokio::test]
async fn an_analysis_target_stopped_for_a_persons_scan_goes_back_to_the_front() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(directory.path());
    let runtime = tokio::runtime::Handle::current();
    assert!(quality_backfill::try_begin(&ctx.quality_backfill, false, 2));
    let job = Job::default();
    job.queue().analysis = Some(Analysis {
        ctx: Arc::clone(&ctx),
        targets: VecDeque::from([2]),
        force: false,
        fill_metadata: false,
        runtime: runtime.clone(),
    });
    let stop = Stop::new();
    stop.request();
    run_analysis_target(&state, &job, &ctx, 1, false, false, &runtime, &stop);
    assert_eq!(
        job.queue().analysis.as_ref().unwrap().targets,
        VecDeque::from([1, 2]),
        "scanned again after the person's scan"
    );
    assert_eq!(
        quality_backfill::snapshot(&ctx.quality_backfill).processed_targets,
        0,
        "not counted as done"
    );
    // Not stopped: the target counts as done.
    run_analysis_target(&state, &job, &ctx, 1, false, false, &runtime, &Stop::new());
    assert_eq!(
        quality_backfill::snapshot(&ctx.quality_backfill).processed_targets,
        1
    );
}

#[tokio::test]
async fn a_failed_runner_finishes_the_progress_it_left_running() {
    let directory = tempfile::tempdir().unwrap();
    let (_state, ctx) = catalog(directory.path());
    let runtime = tokio::runtime::Handle::current();
    spatial_scan::mark_preparing(&ctx.spatial_metrics, 1, None, false);
    assert!(quality_backfill::try_begin(&ctx.quality_backfill, false, 2));
    let mut queue = Queue {
        scans: VecDeque::from([PersonScan {
            ctx: Arc::clone(&ctx),
            request: request(1),
            runtime: runtime.clone(),
        }]),
        analysis: Some(Analysis {
            ctx: Arc::clone(&ctx),
            targets: VecDeque::from([2]),
            force: false,
            fill_metadata: false,
            runtime,
        }),
        analysis_running: true,
        running_ctx: Some(Arc::clone(&ctx)),
    };
    QualityScans::runner_failed(&mut queue, &mut ());
    assert!(queue.scans.is_empty() && queue.analysis.is_none());
    assert!(!queue.analysis_running);
    assert!(
        !spatial_scan::progress_snapshot(&ctx.spatial_metrics)
            .0
            .running
    );
    assert!(!quality_backfill::snapshot(&ctx.quality_backfill).running);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_scan_route_queues_and_answers_at_once() {
    let directory = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(directory.path());
    let axum::Json(response) = crate::server::handlers::start_spatial_scan(
        axum::extract::State(Arc::clone(&state)),
        crate::server::extract::DbContext(Arc::clone(&ctx)),
        axum::Json(request(2)),
    )
    .await
    .unwrap();
    let data = response.data.unwrap();
    assert!(data.started);
    assert!(data.progress.running, "a page polls from the answer on");
    assert_eq!(data.progress.target_id, Some(2));
    let job = state.catalog_jobs.quality_scans.job(&scanner(&ctx));
    until("the runner never exited", || !job.queue().has_runner()).await;
    // While the server shuts down, nothing is queued.
    state.shutdown.begin();
    let axum::Json(response) = crate::server::handlers::start_spatial_scan(
        axum::extract::State(Arc::clone(&state)),
        crate::server::extract::DbContext(Arc::clone(&ctx)),
        axum::Json(request(2)),
    )
    .await
    .unwrap();
    assert!(!response.data.unwrap().started);
}
