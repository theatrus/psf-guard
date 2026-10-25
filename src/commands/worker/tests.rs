//! The worker command against a real server's workers routes, with a
//! stand-in rc-astro on both sides.

#![cfg(unix)]

use super::*;
use crate::server::state::AppState;
use crate::server::workers::board::{Board, Timings};
use crate::server::workers::{tokens, Workers};
use seiza_stacking::LinearImage;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

struct Server {
    state: Arc<AppState>,
    address: String,
    secret: String,
    /// Inputs downloaded and outputs uploaded over HTTP.
    transfers: Arc<AtomicUsize>,
    exchange: tempfile::TempDir,
}

async fn server() -> Server {
    server_with_slow_inputs(Duration::ZERO).await
}

/// A server whose input downloads start only after `delay`, as on a slow
/// link.
async fn server_with_slow_inputs(delay: Duration) -> Server {
    let exchange = tempfile::tempdir().unwrap();
    let mut state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
    state.workers = Workers::with_board(Board::with_timings(
        exchange.path().to_path_buf(),
        Timings {
            lease: Duration::from_secs(3),
            no_worker_grace: Duration::from_secs(10),
        },
    ));
    let (secret, record) = tokens::mint("test").unwrap();
    state.workers.configure(
        crate::db_registry::RemoteWorkerSettings {
            tokens: vec![record],
            ..Default::default()
        },
        exchange.path().to_path_buf(),
    );
    let state = Arc::new(state);
    let transfers = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&transfers);
    let router = axum::Router::new()
        .nest(
            "/api/workers/v1",
            crate::server::workers::routes::worker_routes(),
        )
        .layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let counted = Arc::clone(&counted);
                async move {
                    let path = request.uri().path();
                    if path.contains("/inputs/") || path.contains("/outputs/") {
                        counted.fetch_add(1, Ordering::SeqCst);
                    }
                    if path.contains("/inputs/") {
                        tokio::time::sleep(delay).await;
                    }
                    next.run(request).await
                }
            },
        ))
        .with_state(Arc::clone(&state));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    Server {
        state,
        address,
        secret,
        transfers,
        exchange,
    }
}

/// Start a worker against `server`; it stops when the sender is dropped.
fn start_worker(
    server: &Server,
    files: FileAccess,
    work_dir: &Path,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<()>>,
) {
    start_worker_taking(server, files, work_dir, WorkChoice::default())
}

/// Start a worker that takes only `work`.
fn start_worker_taking(
    server: &Server,
    files: FileAccess,
    work_dir: &Path,
    work: WorkChoice,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<Result<()>>,
) {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let options = WorkerOptions {
        server: server.address.clone(),
        token: server.secret.clone(),
        name: "test-worker".into(),
        files,
        work_dir: work_dir.to_path_buf(),
        tools: LocalTools {
            rc_astro: crate::remote_work::rc_astro::locate(None),
            slots: 1,
        },
        work,
        slots: 1,
    };
    let handle = tokio::spawn(serve(options, async {
        let _ = stopped.await;
    }));
    (stop, handle)
}

async fn eventually(what: &str, mut ready: impl FnMut() -> bool) {
    for _ in 0..200 {
        if ready() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("{what} did not happen");
}

fn connected(server: &Server) -> Option<crate::server::workers::board::SessionSummary> {
    server
        .state
        .workers
        .board
        .summary()
        .0
        .into_iter()
        .find(|worker| worker.name == "test-worker" && worker.connected)
}

fn noise_x() -> seiza_stacking::ExternalToolSchema {
    let cli = crate::remote_work::rc_astro::locate(None).unwrap();
    crate::remote_work::rc_astro::probe(&cli)
        .0
        .into_iter()
        .find(|schema| schema.key == "nxt")
        .unwrap()
}

fn request() -> seiza_stacking::ExternalToolRequest {
    seiza_stacking::ExternalToolRequest {
        tool: "nxt".into(),
        parameters: Default::default(),
        device: None,
    }
}

fn image() -> LinearImage {
    LinearImage::new(
        4,
        2,
        1,
        vec![10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0],
    )
    .unwrap()
}

/// Run NoiseXTerminator on the board, as the server's processing would.
async fn run_step(
    server: &Server,
    cancel: Option<seiza_stacking::CancelSignal>,
) -> (
    Result<seiza_stacking::ProcessedStackImage, String>,
    Vec<Option<String>>,
) {
    let board = Arc::clone(&server.state.workers.board);
    tokio::task::spawn_blocking(move || {
        let mut heard = Vec::new();
        let ran = crate::server::workers::rc_astro::run_on_board(
            &board,
            true,
            &noise_x(),
            &request(),
            &image(),
            &[],
            cancel.as_ref(),
            &mut |_, worker| heard.push(worker.map(str::to_string)),
        );
        (ran, heard)
    })
    .await
    .unwrap()
}

fn assert_same_pixels(processed: &LinearImage) {
    for (got, sent) in processed.data.iter().zip(&image().data) {
        assert!((got - sent).abs() < 1e-2, "{got} != {sent}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_remote_worker_downloads_the_input_and_uploads_the_result() {
    let tool = crate::server::stack_preview::rc_astro::test_tool::install(false);
    let server = server().await;
    let work = tempfile::tempdir().unwrap();
    let (stop, worker) = start_worker(&server, FileAccess::default(), work.path());
    eventually("the worker connects", || connected(&server).is_some()).await;
    let summary = connected(&server).unwrap();
    assert_eq!(summary.rc_astro.len(), 3, "{:?}", summary.rc_astro);
    assert!(!summary.exchange_shared);

    let (ran, heard) = run_step(&server, None).await;
    let processed = ran.unwrap();
    assert_same_pixels(&processed.image);
    assert!(heard.contains(&Some("test-worker".into())), "{heard:?}");
    assert_eq!(tool.invocations(), 1);
    // One download and one upload.
    assert_eq!(server.transfers.load(Ordering::SeqCst), 2);
    eventually("the worker clears its scratch folder", || {
        std::fs::read_dir(work.path()).is_ok_and(|mut entries| entries.next().is_none())
    })
    .await;

    drop(stop);
    worker.await.unwrap().unwrap();
    eventually("the worker leaves", || connected(&server).is_none()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_advertises_only_the_tools_its_work_choices_list() {
    let _tool = crate::server::stack_preview::rc_astro::test_tool::install(false);
    let server = server().await;
    let work = tempfile::tempdir().unwrap();
    let (stop, worker) = start_worker_taking(
        &server,
        FileAccess::default(),
        work.path(),
        WorkChoice::parse(&["bxt".into()]).unwrap(),
    );
    eventually("the worker connects", || connected(&server).is_some()).await;
    let summary = connected(&server).unwrap();
    assert_eq!(summary.rc_astro, ["RC-Astro BlurXTerminator 9.9.9-test"]);
    // NoiseXTerminator is not this worker's work: the step waits for another
    // worker and fails when none connects.
    let (ran, _) = run_step(&server, None).await;
    assert_eq!(
        ran.unwrap_err(),
        "No connected worker offers RC-Astro NoiseXTerminator 9.9.9-test"
    );
    drop(stop);
    worker.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_worker_reads_and_writes_through_its_map() {
    let tool = crate::server::stack_preview::rc_astro::test_tool::install(false);
    let server = server().await;
    let work = tempfile::tempdir().unwrap();
    // The worker sees the exchange folder at another path.
    let mount = tempfile::tempdir().unwrap();
    let seen_at = mount.path().join("exchange");
    std::os::unix::fs::symlink(server.exchange.path(), &seen_at).unwrap();
    let files = FileAccess {
        mode: FileMode::Shared,
        maps: vec![PathMap {
            server: server.exchange.path().display().to_string(),
            local: seen_at.display().to_string(),
        }],
    };
    let (stop, worker) = start_worker(&server, files, work.path());
    eventually("the worker shares the exchange folder", || {
        connected(&server).is_some_and(|worker| worker.exchange_shared)
    })
    .await;

    let (ran, _) = run_step(&server, None).await;
    assert_same_pixels(&ran.unwrap().image);
    assert_eq!(tool.invocations(), 1);
    assert_eq!(
        server.transfers.load(Ordering::SeqCst),
        0,
        "no file went through the server"
    );
    drop(stop);
    worker.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_shared_worker_that_cannot_see_the_folder_falls_back_to_the_server() {
    let tool = crate::server::stack_preview::rc_astro::test_tool::install(false);
    let server = server().await;
    let work = tempfile::tempdir().unwrap();
    let files = FileAccess {
        mode: FileMode::Shared,
        maps: vec![PathMap {
            server: server.exchange.path().display().to_string(),
            local: "/nonexistent/psf-guard-exchange".into(),
        }],
    };
    let (stop, worker) = start_worker(&server, files, work.path());
    eventually("the worker connects", || connected(&server).is_some()).await;
    eventually("the probe is answered", || {
        connected(&server).is_some_and(|worker| worker.exchange_problem.is_some())
    })
    .await;

    let (ran, _) = run_step(&server, None).await;
    assert_same_pixels(&ran.unwrap().image);
    assert_eq!(tool.invocations(), 1);
    assert_eq!(server.transfers.load(Ordering::SeqCst), 2);
    drop(stop);
    worker.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_on_the_server_kills_the_tool_on_the_worker() {
    let tool = crate::server::stack_preview::rc_astro::test_tool::install(true);
    let server = server().await;
    let work = tempfile::tempdir().unwrap();
    let (stop, worker) = start_worker(&server, FileAccess::default(), work.path());
    eventually("the worker connects", || connected(&server).is_some()).await;

    let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let cancel = {
        let cancelled = Arc::clone(&cancelled);
        seiza_stacking::CancelSignal::new(move || cancelled.load(Ordering::SeqCst))
    };
    let step = {
        let server_board = Arc::clone(&server.state.workers.board);
        tokio::task::spawn_blocking(move || {
            crate::server::workers::rc_astro::run_on_board(
                &server_board,
                true,
                &noise_x(),
                &request(),
                &image(),
                &[],
                Some(&cancel),
                &mut |_, _| {},
            )
        })
    };
    eventually("the tool starts on the worker", || tool.pid().is_some()).await;
    let pid = tool.pid().unwrap();
    cancelled.store(true, Ordering::SeqCst);
    let refused = step.await.unwrap().unwrap_err();
    assert!(refused.contains("was stopped"), "{refused}");
    // The worker hears it at its next report, a second later here.
    eventually("the worker kills the tool", || {
        !std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    })
    .await;
    eventually("the worker is free again", || {
        connected(&server).is_some_and(|worker| worker.running.is_empty())
    })
    .await;
    drop(stop);
    worker.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_worker_with_a_revoked_token_stops_with_the_reason() {
    let server = server().await;
    server.state.workers.set_tokens(Vec::new());
    let work = tempfile::tempdir().unwrap();
    let (_stop, worker) = start_worker(&server, FileAccess::default(), work.path());
    let error = tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("Settings › Workers"), "{error}");
}

#[test]
fn maps_imply_shared_files_and_must_name_both_sides() {
    let args = |maps: &[&str]| WorkerArgs {
        server: "http://server:3000/".into(),
        token: Some("psfw_x".into()),
        name: Some("n".into()),
        files: WorkerFilesArg::Remote,
        maps: maps.iter().map(|map| map.to_string()).collect(),
        work_dir: None,
        rc_astro: None,
        work: Vec::new(),
        slots: 1,
    };
    let remote = options(args(&[])).unwrap();
    assert_eq!(remote.files.mode, FileMode::Remote);
    assert_eq!(remote.server, "http://server:3000");
    let shared = options(args(&["/srv=/mnt"])).unwrap();
    assert_eq!(shared.files.mode, FileMode::Shared);
    assert_eq!(
        shared.files.maps,
        vec![PathMap {
            server: "/srv".into(),
            local: "/mnt".into()
        }]
    );
    assert!(options(args(&["/srv"])).is_err());
    assert!(options(args(&["=/mnt"])).is_err());
}

/// The real rc-astro on a remote worker, over HTTP, in both file modes.
/// Needs a licensed rc-astro on this machine:
/// `cargo test --lib real_rc_astro -- --ignored`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "needs a licensed rc-astro"]
async fn real_rc_astro_runs_on_a_remote_worker() {
    let cli = crate::remote_work::rc_astro::locate(None).expect("rc-astro is installed");
    let schema = crate::remote_work::rc_astro::probe(&cli)
        .0
        .into_iter()
        .find(|schema| schema.key == "nxt" && schema.licensed)
        .expect("NoiseXTerminator is licensed");
    // A 1024×1024 sky: a sloped background, noise and a grid of stars.
    let (width, height) = (1024usize, 1024usize);
    let mut seed = 7u64;
    let mut noise = move || {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
    };
    let mut data = vec![0.0f32; width * height];
    for y in 0..height {
        for x in 0..width {
            let mut value = 500.0 + x as f32 * 0.05 + noise() * 40.0;
            let (sx, sy) = ((x % 64) as f32 - 32.0, (y % 64) as f32 - 32.0);
            value += 8000.0 * (-(sx * sx + sy * sy) / 4.0).exp();
            data[y * width + x] = value;
        }
    }
    let image = LinearImage::new(width, height, 1, data).unwrap();
    for files in [
        FileAccess::default(),
        FileAccess {
            mode: FileMode::Shared,
            maps: Vec::new(),
        },
    ] {
        let server = server().await;
        let work = tempfile::tempdir().unwrap();
        let shared = files.mode == FileMode::Shared;
        let (stop, worker) = start_worker(&server, files, work.path());
        eventually("the worker connects", || {
            connected(&server).is_some_and(|worker| !shared || worker.exchange_shared)
        })
        .await;
        let board = Arc::clone(&server.state.workers.board);
        let (schema, sent) = (schema.clone(), image.clone());
        let started = std::time::Instant::now();
        let processed = tokio::task::spawn_blocking(move || {
            crate::server::workers::rc_astro::run_on_board(
                &board,
                true,
                &schema,
                &seiza_stacking::ExternalToolRequest {
                    tool: "nxt".into(),
                    parameters: Default::default(),
                    device: None,
                },
                &sent,
                &[],
                None,
                &mut |fraction, worker| tracing::info!("{:.0}% on {worker:?}", fraction * 100.0),
            )
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            (processed.image.width, processed.image.height),
            (width, height)
        );
        let spread = |data: &[f32]| {
            let mean = data.iter().sum::<f32>() / data.len() as f32;
            (data.iter().map(|value| (value - mean).powi(2)).sum::<f32>() / data.len() as f32)
                .sqrt()
        };
        // A background patch between stars is quieter after NoiseX.
        let patch = |image: &LinearImage| {
            (480usize..540)
                .flat_map(|y| (480usize..540).map(move |x| (x, y)))
                .filter(|(x, y)| (x % 64).abs_diff(32) > 12 && (y % 64).abs_diff(32) > 12)
                .map(|(x, y)| image.data[y * width + x])
                .collect::<Vec<_>>()
        };
        let (before, after) = (spread(&patch(&image)), spread(&patch(&processed.image)));
        eprintln!(
            "{} files: NoiseX in {:.1}s on {:?}, background spread {before:.1} → {after:.1}, transfers {}",
            if shared { "shared" } else { "remote" },
            started.elapsed().as_secs_f32(),
            processed.device,
            server.transfers.load(Ordering::SeqCst)
        );
        assert!(after < before, "{after} >= {before}");
        assert_eq!(
            server.transfers.load(Ordering::SeqCst),
            if shared { 0 } else { 2 }
        );
        drop(stop);
        worker.await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slow_download_keeps_its_lease() {
    let tool = crate::server::stack_preview::rc_astro::test_tool::install(false);
    // The download takes longer than the three-second lease.
    let server = server_with_slow_inputs(Duration::from_secs(5)).await;
    let work = tempfile::tempdir().unwrap();
    let (stop, worker) = start_worker(&server, FileAccess::default(), work.path());
    eventually("the worker connects", || connected(&server).is_some()).await;
    let (ran, _) = run_step(&server, None).await;
    assert_same_pixels(&ran.unwrap().image);
    assert_eq!(tool.invocations(), 1, "the task ran once, under one lease");
    drop(stop);
    worker.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoking_the_token_stops_the_running_tool() {
    let tool = crate::server::stack_preview::rc_astro::test_tool::install(true);
    let server = server().await;
    let work = tempfile::tempdir().unwrap();
    let (_stop, worker) = start_worker(&server, FileAccess::default(), work.path());
    eventually("the worker connects", || connected(&server).is_some()).await;
    let board = Arc::clone(&server.state.workers.board);
    let step = tokio::task::spawn_blocking(move || {
        crate::server::workers::rc_astro::run_on_board(
            &board,
            true,
            &noise_x(),
            &request(),
            &image(),
            &[],
            None,
            &mut |_, _| {},
        )
    });
    eventually("the tool starts on the worker", || tool.pid().is_some()).await;
    let pid = tool.pid().unwrap();
    server.state.workers.set_tokens(Vec::new());
    // At its next report the worker learns the token is revoked: it stops
    // the tool and exits with the reason.
    let error = tokio::time::timeout(Duration::from_secs(20), worker)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("Settings › Workers"), "{error}");
    assert!(!std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success());
    drop(step);
}

#[test]
fn a_task_id_or_file_name_that_leaves_the_work_folder_is_refused() {
    use crate::remote_work::protocol::{InputFile, OutputFile, TaskKind};
    let assignment = |task: &str, input: &str, output: &str| Assignment {
        task: task.into(),
        lease: "l".into(),
        kind: TaskKind::RcAstroStep,
        label: "t".into(),
        spec: serde_json::json!({}),
        inputs: vec![InputFile {
            name: input.into(),
            size: 1,
            path: None,
        }],
        outputs: vec![OutputFile {
            name: output.into(),
            required: true,
            path: None,
        }],
    };
    let id = uuid::Uuid::new_v4().to_string();
    assert!(check_assignment(&assignment(&id, "input.fits", "processed.fits")).is_ok());
    for (task, input, output) in [
        ("../../..", "input.fits", "processed.fits"),
        (&id.to_uppercase(), "input.fits", "processed.fits"),
        (&id, "../../.ssh/authorized_keys", "processed.fits"),
        (&id, "input.fits", "/etc/passwd"),
    ] {
        assert!(
            check_assignment(&assignment(task, input, output)).is_err(),
            "{task} {input} {output}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_stacks_a_channel_to_the_same_pixels_as_this_server() {
    use crate::server::stack_preview::remote_channel::testing;
    let frames = tempfile::tempdir().unwrap();
    let group = testing::synthetic_group(frames.path());
    let here = tempfile::tempdir().unwrap();
    let local = {
        let (group, directory) = (group.clone(), here.path().to_path_buf());
        tokio::task::spawn_blocking(move || testing::stack_here(&group, &directory))
            .await
            .unwrap()
    };
    assert_eq!(
        local.dispositions,
        ["reference", "accepted", "accepted", "accepted", "accepted"],
        "{:?}",
        local.notes
    );

    for files in [
        FileAccess::default(),
        FileAccess {
            mode: FileMode::Shared,
            maps: Vec::new(),
        },
    ] {
        let shared = files.mode == FileMode::Shared;
        let server = server().await;
        // Stack builds stay on the server unless Settings › Workers sends
        // them out.
        server.state.workers.board.set_placement(
            crate::remote_work::protocol::TaskKind::StackChannel,
            crate::server::workers::board::WorkPlacement::WorkersOnly,
        );
        let work = tempfile::tempdir().unwrap();
        let (stop, worker) = start_worker_taking(
            &server,
            files,
            work.path(),
            WorkChoice::parse(&["stack".into()]).unwrap(),
        );
        eventually("the worker connects", || {
            connected(&server).is_some_and(|worker| !shared || worker.exchange_shared)
        })
        .await;
        let there = tempfile::tempdir().unwrap();
        let remote = {
            let (state, group, directory) = (
                Arc::clone(&server.state),
                group.clone(),
                there.path().to_path_buf(),
            );
            tokio::task::spawn_blocking(move || {
                testing::stack_on_worker(&state, &group, &directory)
            })
            .await
            .unwrap()
        };
        assert_eq!(remote.dispositions, local.dispositions);
        assert_eq!(remote.matched_stars, local.matched_stars);
        assert_eq!(remote.pixels, local.pixels, "the same stack, bit for bit");
        assert!(remote.checkpoint, "the checkpoint came back");
        assert!(
            remote.notes.contains(&"Stacked on test-worker".to_string()),
            "{:?}",
            remote.notes
        );
        // The card showed the worker's running image; the last one went
        // with the final events, before the outputs.
        assert!(
            remote.images.last() == Some(&5) && remote.images.iter().all(|frames| *frames <= 5),
            "{:?}",
            remote.images
        );
        // Every frame went over HTTP in remote mode: five frames, then the
        // stack, the clipped map and the checkpoint's two halves.
        let transfers = server.transfers.load(Ordering::SeqCst);
        if shared {
            assert_eq!(transfers, 0);
        } else {
            assert!(transfers >= 8, "{transfers}");
        }
        drop(stop);
        worker.await.unwrap().unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_keeps_a_running_image_only_from_the_current_lease_and_only_a_png() {
    let server = server().await;
    server.state.workers.board.set_placement(
        crate::remote_work::protocol::TaskKind::StackChannel,
        crate::server::workers::board::WorkPlacement::WorkersOnly,
    );
    let ticket = server.state.workers.board.ticket().unwrap();
    ticket
        .submit(crate::server::workers::NewTask {
            kind: crate::remote_work::protocol::TaskKind::StackChannel,
            label: "Stack".into(),
            person: true,
            needs: crate::server::workers::TaskNeeds::Stack {
                stacking_version: crate::server::stack_preview::SEIZA_STACKING_OUTPUT_VERSION
                    .into(),
                release: env!("CARGO_PKG_VERSION").into(),
            },
            spec: serde_json::json!({}),
            inputs: Vec::new(),
            outputs: Vec::new(),
        })
        .unwrap();
    let client = Client::new(&server.address, &server.secret).unwrap();
    let (offers, _) = LocalTools {
        rc_astro: None,
        slots: 1,
    }
    .offers(1, &WorkChoice::parse(&["stack".into()]).unwrap());
    let session = client
        .register(&Registration {
            protocol: PROTOCOL_VERSION,
            name: "test-worker".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            offers,
            files: FileAccess::default(),
        })
        .await
        .unwrap()
        .session;
    let assignment = client
        .claim(&session, Duration::from_secs(1))
        .await
        .unwrap()
        .expect("the task");

    let mut png = Vec::new();
    image::RgbImage::new(4, 3)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();
    let mut huge = png.clone();
    huge[16..20].copy_from_slice(&5000u32.to_be_bytes());
    let mut lost = assignment.clone();
    lost.lease = "an earlier lease".into();
    for (assignment, image, refused) in [
        (&assignment, b"not a PNG".to_vec(), Some("not a PNG")),
        (&assignment, huge, Some("larger than 4096")),
        (&lost, png.clone(), Some("")),
        (&assignment, png.clone(), None),
    ] {
        let sent = client.preview(&session, assignment, image, 2).await;
        match refused {
            Some(why) => assert!(
                sent.as_ref()
                    .is_err_and(|error| error.to_string().contains(why)),
                "{:?}",
                sent.map_err(|error| error.to_string())
            ),
            None => sent.unwrap(),
        }
    }
    client
        .complete(
            &session,
            &assignment,
            &Completion {
                result: serde_json::json!({}),
                outputs: Vec::new(),
            },
        )
        .await
        .unwrap();
    let mut received = Vec::new();
    tokio::task::block_in_place(|| {
        ticket.wait_with(crate::server::workers::Waiting {
            stopped: &|| false,
            progress: &mut |_, _| {},
            events: &mut |_| {},
            preview: &mut |image| received.push((image.png.to_vec(), image.frames)),
            finish_on_stop: None,
        })
    })
    .unwrap();
    assert_eq!(received, [(png, 2)]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_worker_stacks_a_mosaic_to_the_same_pixels_as_this_server() {
    use crate::server::stack_preview::remote_channel::testing;
    let frames = tempfile::tempdir().unwrap();
    let mosaic = testing::synthetic_mosaic(frames.path());
    let here = tempfile::tempdir().unwrap();
    let local = {
        let (mosaic, directory) = (mosaic.clone(), here.path().to_path_buf());
        tokio::task::spawn_blocking(move || testing::stack_here(&mosaic, &directory))
            .await
            .unwrap()
    };
    assert_eq!(
        local
            .dispositions
            .iter()
            .filter(|d| *d == "accepted")
            .count(),
        8,
        "{:?} {:?}",
        local.dispositions,
        local.notes
    );

    for files in [
        FileAccess::default(),
        FileAccess {
            mode: FileMode::Shared,
            maps: Vec::new(),
        },
    ] {
        let shared = files.mode == FileMode::Shared;
        let server = server().await;
        server.state.workers.board.set_placement(
            crate::remote_work::protocol::TaskKind::StackChannel,
            crate::server::workers::board::WorkPlacement::WorkersOnly,
        );
        let work = tempfile::tempdir().unwrap();
        let (stop, worker) = start_worker_taking(
            &server,
            files,
            work.path(),
            WorkChoice::parse(&["stack".into()]).unwrap(),
        );
        eventually("the worker connects", || {
            connected(&server).is_some_and(|worker| !shared || worker.exchange_shared)
        })
        .await;
        let there = tempfile::tempdir().unwrap();
        // A capture-order checkpoint of this target and filter, which a
        // mosaic does not replace.
        for name in ["resume.ctx", "resume.json"] {
            std::fs::write(there.path().join(name), name).unwrap();
        }
        let remote = {
            let (state, mosaic, directory) = (
                Arc::clone(&server.state),
                mosaic.clone(),
                there.path().to_path_buf(),
            );
            tokio::task::spawn_blocking(move || {
                testing::stack_on_worker(&state, &mosaic, &directory)
            })
            .await
            .unwrap()
        };
        assert_eq!(remote.dispositions, local.dispositions);
        assert_eq!(remote.pixels, local.pixels, "the same mosaic, bit for bit");
        for name in ["resume.ctx", "resume.json"] {
            assert_eq!(
                std::fs::read_to_string(there.path().join(name)).unwrap(),
                name,
                "a mosaic leaves this server's checkpoint alone"
            );
        }
        drop(stop);
        worker.await.unwrap().unwrap();
    }
}
