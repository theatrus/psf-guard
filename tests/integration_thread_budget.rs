//! A budgeted background job's Seiza work stays on the threads the job was
//! given.
//!
//! Seiza does its parallel work in the Rayon pool of the thread that calls
//! it, and Rayon's global pool has a thread for every core. Each job here
//! runs through the helper the server uses for it, and the global pool must
//! never start: work that left the job's pool would start it. Previews a
//! person waits for use every core on purpose, so they are not here. That can be
//! checked once per process, and each file under `tests/` runs as a process
//! of its own, so this is one test.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use psf_guard::concurrency::ComputePool;
use psf_guard::preview_format::PreviewEncoding;
use psf_guard::server::preview_queue::{GenJob, GenKind};
use psf_guard::server::spatial_scan::{ScanWorkItem, SpatialMetricsStore};
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn budgeted_jobs_keep_seiza_inside_their_pools() {
    let dir = tempfile::tempdir().unwrap();
    let mono = dir.path().join("mono.fits");
    let mosaic = dir.path().join("mosaic.fits");
    write_star_field(&mono, None);
    write_star_field(&mosaic, Some("RGGB"));

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
    let scanned = mono;
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

    rayon::ThreadPoolBuilder::new()
        .build_global()
        .expect("a job's Seiza work left its pool and started the global pool");
}
