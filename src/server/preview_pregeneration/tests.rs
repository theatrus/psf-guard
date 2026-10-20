use super::*;
use crate::preview_format::PreviewFormat;
use std::io::Write;

fn write_fits(path: &std::path::Path) {
    let mut header = Vec::new();
    for card in [
        "SIMPLE  =                    T",
        "BITPIX  =                   16",
        "NAXIS   =                    2",
        "NAXIS1  =                   16",
        "NAXIS2  =                   16",
        "IMAGETYP= 'LIGHT'",
        "END",
    ] {
        let mut bytes = card.as_bytes().to_vec();
        bytes.resize(80, b' ');
        header.extend_from_slice(&bytes);
    }
    header.resize(header.len().div_ceil(2880) * 2880, b' ');
    let mut data = (0..256i16)
        .flat_map(|value| (value * 37).to_be_bytes())
        .collect::<Vec<_>>();
    data.resize(2880, 0);
    let mut file = std::fs::File::create(path).unwrap();
    file.write_all(&header).unwrap();
    file.write_all(&data).unwrap();
}

/// A catalog with one light frame on disk, and a server that pre-generates
/// screen previews as JPEG.
fn catalog(temp: &std::path::Path) -> (Arc<AppState>, Arc<DatabaseContext>) {
    let database = temp.join("scheduler.sqlite");
    crate::ts_schema::create_fresh_db(&database).unwrap();
    let images = temp.join("images");
    std::fs::create_dir_all(&images).unwrap();
    write_fits(&images.join("frame.fits"));
    let ctx = DatabaseContext::new(
        "test".into(),
        "Test".into(),
        database.to_string_lossy().into_owned(),
        vec![images.to_string_lossy().into_owned()],
        None,
        None,
        None,
        temp.join("cache").to_string_lossy().into_owned(),
    )
    .unwrap();
    {
        let connection = ctx.db();
        let connection = connection.lock().unwrap();
        connection
            .execute_batch(
                "INSERT INTO project (Id, profileId, name, isMosaic, flatsHandling)
                     VALUES (1, 'profile', 'Project', 0, 0);
                 INSERT INTO target (Id, name, active, epochcode, projectId)
                     VALUES (1, 'Target', 1, 0, 1);
                 INSERT INTO acquiredimage
                    (Id, projectId, targetId, acquireddate, filtername, gradingStatus,
                     metadata, profileId)
                 VALUES (1, 1, 1, 1700000000, 'L', 0,
                         '{\"FileName\": \"C:\\\\lights\\\\frame.fits\"}', 'profile');",
            )
            .unwrap();
    }
    let mut state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
    state.pregeneration_config.screen_enabled = true;
    *state.preview_encoding.write().unwrap() = PreviewEncoding {
        format: PreviewFormat::Jpeg,
        ..PreviewEncoding::default()
    };
    // Pre-generation stops as the cache volume nears its limit; the disk
    // the test runs on may be that full.
    state.cache_budget.lift_limit_for_test(&ctx.cache_dir_path);
    let state = Arc::new(state);
    let ctx = Arc::new(ctx);
    state.add_database(Arc::clone(&ctx));
    (state, ctx)
}

fn written(ctx: &DatabaseContext) -> Vec<String> {
    std::fs::read_dir(ctx.cache_dir_path.join("previews"))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .filter(|name| !name.contains(".tmp."))
                .collect()
        })
        .unwrap_or_default()
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let began = Instant::now();
    while !done() {
        assert!(began.elapsed() < Duration::from_secs(20), "{what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A pass writes the file name the viewer asks for. With JPEG configured,
/// pre-generation once wrote `.png`, so the viewer missed and rendered every
/// image again. A second pass finds the preview and has nothing to do.
#[test]
fn a_pass_writes_the_preview_the_viewer_asks_for_once() {
    let temp = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(temp.path());

    let pending = pending_images(&state, &ctx).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].file_only, "frame.fits");
    let rows = image_rows(&ctx, &pending).unwrap();
    let rendered = render(&ctx, &pending[0], &rows[&1], state.preview_encoding());
    assert_eq!((rendered.written, rendered.failed), (1, 0));
    let names = written(&ctx);
    assert_eq!(names.len(), 1, "{names:?}");
    assert!(names[0].ends_with(".jpg"), "{names:?}");

    assert!(pending_images(&state, &ctx).unwrap().is_empty());
}

/// The runner writes the catalog's previews and reports them in its status.
#[test]
fn a_requested_pass_runs_on_the_catalog_runner() {
    let temp = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(temp.path());

    request(&state, &ctx.id);
    wait_until("the pass finishes", || {
        let status = status(&state, &ctx.id);
        !status.active && status.done == 1
    });
    let status = status(&state, &ctx.id);
    assert_eq!((status.total, status.written, status.failed), (1, 1, 0));
    assert_eq!(written(&ctx).len(), 1);
}

/// A pass waits while a person's work runs, and the status shows what for.
/// Stop ends it without writing, and no further pass runs.
#[test]
fn a_pass_waits_for_a_persons_work_and_stop_ends_it() {
    let temp = tempfile::tempdir().unwrap();
    let (state, ctx) = catalog(temp.path());
    let person = state.begin_named_interactive_job("a stack build");

    request(&state, &ctx.id);
    wait_until("the pass waits for the build", || {
        status(&state, &ctx.id).waiting_for.as_deref() == Some("a stack build")
    });
    assert_eq!(status(&state, &ctx.id).total, 1);

    assert!(stop(&state, &ctx.id));
    wait_until("the pass ends", || !status(&state, &ctx.id).active);
    assert_eq!(status(&state, &ctx.id).written, 0);
    assert!(written(&ctx).is_empty());

    drop(person);
    request(&state, &ctx.id);
    let job = state.catalog_jobs.pregeneration.job(&ctx.id);
    assert!(!job.queue().has_runner(), "no pass runs after Stop");
    assert!(!stop(&state, &ctx.id), "nothing runs to stop");
}
