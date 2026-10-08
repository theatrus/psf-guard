//! A budgeted job's Seiza work stays on the threads the job was given.
//!
//! Seiza does its parallel work in the Rayon pool of the thread that calls
//! it, and Rayon's global pool has a thread for every core. Each job here
//! runs through the helper the server uses for it, and the global pool must
//! never start: work that left the job's pool would start it. That can be
//! checked once per process, and each file under `tests/` runs as a process
//! of its own, so this is one test.

use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use psf_guard::concurrency::ComputePool;
use psf_guard::preview_format::PreviewEncoding;
use psf_guard::server::preview_queue::{GenJob, GenKind, GenerationState};
use psf_guard::server::spatial_scan::{ScanWorkItem, SpatialMetricsStore};
use psf_guard::server::state::AppState;
use seiza_fits::{F32ImageData, HeaderValue, WriteHeaderCard};

const WIDTH: usize = 640;
const HEIGHT: usize = 480;

/// A star field large enough for Seiza's parallel paths to split, as a
/// mosaic when `bayer` names a pattern.
fn write_star_field(path: &Path, bayer: Option<&str>) {
    let stars = (0..60)
        .map(|index| {
            let x = ((index * 7919) % 997) as f32 / 997.0 * (WIDTH as f32 - 40.0) + 20.0;
            let y = ((index * 6271) % 991) as f32 / 991.0 * (HEIGHT as f32 - 40.0) + 20.0;
            (x, y, 8000.0 + ((index * 37) % 41) as f32 * 400.0)
        })
        .collect::<Vec<_>>();
    let pixels = (0..WIDTH * HEIGHT)
        .map(|index| {
            let (x, y) = ((index % WIDTH) as f32, (index / WIDTH) as f32);
            let mut value = 1200.0 + ((index * 17 + index / WIDTH * 31) % 23) as f32 * 2.0;
            for (star_x, star_y, brightness) in &stars {
                let r2 = (x - star_x).powi(2) + (y - star_y).powi(2);
                if r2 < 60.0 {
                    value += brightness * (-r2 / 4.5).exp();
                }
            }
            value.round()
        })
        .collect::<Vec<f32>>();
    let mut cards = vec![
        WriteHeaderCard::new("IMAGETYP", HeaderValue::String("LIGHT".into())),
        WriteHeaderCard::new("EXPTIME", HeaderValue::Float(120.0)),
    ];
    if let Some(pattern) = bayer {
        cards.push(WriteHeaderCard::new(
            "BAYERPAT",
            HeaderValue::String(pattern.into()),
        ));
    }
    seiza_fits::write_f32_image(path, WIDTH, HEIGHT, F32ImageData::Mono(&pixels), &cards).unwrap();
}

fn preview(fits_path: &Path, cache_path: PathBuf, color: bool) -> GenJob {
    GenJob {
        fits_path: fits_path.to_path_buf(),
        cache_path,
        kind: GenKind::Preview {
            midtone: 0.2,
            shadow: -2.8,
            max_dimensions: Some((320, 320)),
            color,
        },
        encoding: PreviewEncoding::default(),
    }
}

/// Wait for the preview queue to finish `job`, as the viewer's poll would.
async fn generated(state: &AppState, job: &GenJob) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let status = state
            .preview_queue
            .status_for_source(&job.cache_path, &job.fits_path);
        match status.map(|status| status.state) {
            Some(GenerationState::Ready) => return,
            Some(GenerationState::Error) => panic!("{} failed", job.cache_path.display()),
            _ => {}
        }
        assert!(
            Instant::now() < deadline,
            "{} timed out",
            job.cache_path.display()
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budgeted_jobs_keep_seiza_inside_their_pools() {
    let dir = tempfile::tempdir().unwrap();
    let mono = dir.path().join("mono.fits");
    let mosaic = dir.path().join("mosaic.fits");
    write_star_field(&mono, None);
    write_star_field(&mosaic, Some("RGGB"));
    let state = Arc::new(AppState::new_for_test(
        rusqlite::Connection::open_in_memory().unwrap(),
    ));

    // Interactive previews, through the queue: a stretch, a debayer and a
    // star annotation.
    let jobs = [
        preview(&mono, dir.path().join("mono.png"), false),
        preview(&mosaic, dir.path().join("mosaic.png"), true),
        GenJob {
            fits_path: mono.clone(),
            cache_path: dir.path().join("annotated.png"),
            kind: GenKind::Annotated {
                max_stars: 40,
                size: "screen".into(),
            },
            encoding: PreviewEncoding::default(),
        },
    ];
    for job in &jobs {
        state.enqueue_preview(job.clone());
    }
    for job in &jobs {
        generated(&state, job).await;
    }

    // Background pre-generation renders in a pool of its leased workers.
    let background = preview(&mosaic, dir.path().join("background.png"), true);
    tokio::task::spawn_blocking(move || {
        let pool = ComputePool::take("pregeneration", 2).unwrap();
        pool.install(|| psf_guard::server::preview_queue::generate(&background))
            .unwrap();
    })
    .await
    .unwrap();

    // A quality scan's spatial stage, which detects stars in each frame.
    let cache = dir.path().join("cache");
    std::fs::create_dir_all(&cache).unwrap();
    let scanned = mono.clone();
    let metrics = tokio::task::spawn_blocking(move || {
        let store = RwLock::new(SpatialMetricsStore::default());
        let item = ScanWorkItem {
            image_id: 1,
            filename: "mono.fits".into(),
            fits_path: scanned,
            source_generation: 0,
            source_revision: None,
        };
        psf_guard::server::spatial_scan::run_scan(&store, &cache, &[item], 2, &|| {});
        let store = store.into_inner().unwrap();
        (store.metrics.len(), store.progress.errors)
    })
    .await
    .unwrap();
    assert_eq!(metrics, (1, 0));

    // An on-demand request, in the pool interactive work shares.
    let pools = Arc::clone(&state);
    let stars = tokio::task::spawn_blocking(move || {
        pools.run_interactive(|| {
            let fits = psf_guard::image_analysis::FitsImage::from_file(&mono).unwrap();
            let params = psf_guard::hocus_focus_star_detection::HocusFocusParams::default();
            psf_guard::hocus_focus_star_detection::detect_stars_hocus_focus(
                &fits.data,
                fits.width,
                fits.height,
                &params,
            )
            .stars
            .len()
        })
    })
    .await
    .unwrap();
    assert!(stars > 0);

    rayon::ThreadPoolBuilder::new()
        .build_global()
        .expect("a job's Seiza work left its pool and started the global pool");
}
