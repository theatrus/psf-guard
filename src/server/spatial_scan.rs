//! Server-side spatial-metrics scanning.
//!
//! Computes the grid-based occlusion metrics from `spatial_analysis` for the
//! FITS files behind a target's acquired images, as a background task with
//! pollable progress (same pattern as the file-cache refresh). Results are
//! held in memory per `DatabaseContext` and persisted as JSON in the per-DB
//! cache directory, so a scan survives server restarts and the sequence
//! analysis endpoint can merge the metrics without recomputing.
//!
//! Star detection on a full-frame image takes seconds, which is why this is
//! a scan-once-then-cache design rather than compute-on-request.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::FitsImage;
use crate::spatial_analysis::{compute_spatial_metrics, PixelCalibration, SpatialAnalysisConfig};

/// Persisted per-image spatial metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSpatialMetrics {
    pub image_id: i32,
    /// Basename of the FITS file the metrics were computed from; a changed
    /// filename invalidates the entry.
    pub filename: String,
    pub star_count: usize,
    pub avg_hfr: f64,
    pub dead_cell_fraction: Option<f64>,
    pub star_uniformity: Option<f64>,
    pub bg_cell_spread: f64,
    pub bg_cell_max_dev: f64,
    pub median_adu: f64,
    /// Epoch seconds when computed.
    pub computed_at: i64,
}

/// Progress of the (singleton per-DB) spatial scan.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SpatialScanProgress {
    pub running: bool,
    pub target_id: Option<i32>,
    pub filter_name: Option<String>,
    pub total: usize,
    pub processed: usize,
    /// Images skipped because a cached entry already existed.
    pub skipped_cached: usize,
    /// Images whose FITS file could not be found or read.
    pub errors: usize,
    pub current_file: Option<String>,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
    pub last_error: Option<String>,
}

/// In-memory store + scan state for one database. Held on `DatabaseContext`.
#[derive(Debug, Default)]
pub struct SpatialMetricsStore {
    pub metrics: HashMap<i32, StoredSpatialMetrics>,
    pub progress: SpatialScanProgress,
    loaded_from_disk: bool,
}

/// One unit of scan work, resolved from the DB before the blocking task runs.
#[derive(Debug, Clone)]
pub struct ScanWorkItem {
    pub image_id: i32,
    pub filename: String,
    pub fits_path: PathBuf,
}

const PERSIST_FILENAME: &str = "spatial_metrics.json";
/// Persist every N processed frames so a crash loses little work.
const PERSIST_EVERY: usize = 5;

fn persist_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join(PERSIST_FILENAME)
}

/// Load persisted metrics from the per-DB cache dir (idempotent).
pub fn ensure_loaded(store: &RwLock<SpatialMetricsStore>, cache_dir: &Path) {
    {
        let s = store.read().unwrap();
        if s.loaded_from_disk {
            return;
        }
    }
    let mut s = store.write().unwrap();
    if s.loaded_from_disk {
        return;
    }
    s.loaded_from_disk = true;

    let path = persist_path(cache_dir);
    let Ok(contents) = std::fs::read_to_string(&path) else {
        return;
    };
    match serde_json::from_str::<Vec<StoredSpatialMetrics>>(&contents) {
        Ok(entries) => {
            tracing::info!(
                "📐 Loaded {} spatial metric entries from {}",
                entries.len(),
                path.display()
            );
            s.metrics = entries.into_iter().map(|e| (e.image_id, e)).collect();
        }
        Err(e) => {
            tracing::warn!(
                "📐 Ignoring unreadable spatial metrics file {}: {}",
                path.display(),
                e
            );
        }
    }
}

fn persist(store: &RwLock<SpatialMetricsStore>, cache_dir: &Path) {
    let entries: Vec<StoredSpatialMetrics> = {
        let s = store.read().unwrap();
        s.metrics.values().cloned().collect()
    };
    let path = persist_path(cache_dir);
    let tmp = path.with_extension("json.tmp");
    let json = match serde_json::to_string(&entries) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("📐 Failed to serialize spatial metrics: {}", e);
            return;
        }
    };
    if let Err(e) = std::fs::write(&tmp, json).and_then(|_| std::fs::rename(&tmp, &path)) {
        tracing::error!(
            "📐 Failed to persist spatial metrics to {}: {}",
            path.display(),
            e
        );
    }
}

/// Try to mark a scan as started. Returns false when one is already running.
pub fn try_begin_scan(
    store: &RwLock<SpatialMetricsStore>,
    target_id: i32,
    filter_name: Option<String>,
    total: usize,
    skipped_cached: usize,
) -> bool {
    let mut s = store.write().unwrap();
    if s.progress.running {
        return false;
    }
    s.progress = SpatialScanProgress {
        running: true,
        target_id: Some(target_id),
        filter_name,
        total,
        skipped_cached,
        started_at: Some(chrono::Utc::now().timestamp()),
        ..Default::default()
    };
    true
}

/// Worker threads for the scan. Detection is CPU-bound at several seconds
/// per full-frame image; two workers roughly halve the wall clock while
/// leaving cores free to keep serving requests.
const SCAN_WORKERS: usize = 2;

/// Run the scan synchronously (call from `spawn_blocking`). `work` must only
/// contain images that actually need computing.
pub fn run_scan(store: &RwLock<SpatialMetricsStore>, cache_dir: &Path, work: &[ScanWorkItem]) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let spatial_config = SpatialAnalysisConfig::default();
    let next = AtomicUsize::new(0);
    let since_persist = AtomicUsize::new(0);
    let workers = SCAN_WORKERS.min(work.len()).max(1);

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= work.len() {
                    break;
                }
                let item = &work[i];
                {
                    let mut s = store.write().unwrap();
                    s.progress.current_file = Some(item.filename.clone());
                }

                match compute_one(item, &spatial_config) {
                    Ok(entry) => {
                        let mut s = store.write().unwrap();
                        s.metrics.insert(item.image_id, entry);
                        s.progress.processed += 1;
                    }
                    Err(e) => {
                        tracing::warn!(
                            "📐 Spatial scan failed for image {} ({}): {}",
                            item.image_id,
                            item.filename,
                            e
                        );
                        let mut s = store.write().unwrap();
                        s.progress.errors += 1;
                        s.progress.processed += 1;
                        s.progress.last_error = Some(format!("{}: {}", item.filename, e));
                    }
                }

                if since_persist.fetch_add(1, Ordering::Relaxed) + 1 >= PERSIST_EVERY {
                    since_persist.store(0, Ordering::Relaxed);
                    persist(store, cache_dir);
                }
            });
        }
    });

    persist(store, cache_dir);
    let mut s = store.write().unwrap();
    s.progress.running = false;
    s.progress.current_file = None;
    s.progress.finished_at = Some(chrono::Utc::now().timestamp());
}

fn compute_one(
    item: &ScanWorkItem,
    config: &SpatialAnalysisConfig,
) -> anyhow::Result<StoredSpatialMetrics> {
    let fits = FitsImage::from_file(&item.fits_path)?;
    let stats = fits.calculate_basic_statistics();

    let params = HocusFocusParams::default();
    let result = detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);
    let positions: Vec<(f64, f64)> = result.stars.iter().map(|s| s.position).collect();

    let calibration = PixelCalibration {
        adu_offset: fits.raw_min + fits.bzero,
        adu_per_stored: 1.0 / fits.raw_scale,
    };
    let spatial = compute_spatial_metrics(
        &positions,
        &fits.data,
        fits.width,
        fits.height,
        &calibration,
        config,
    );

    Ok(StoredSpatialMetrics {
        image_id: item.image_id,
        filename: item.filename.clone(),
        star_count: result.stars.len(),
        avg_hfr: result.average_hfr,
        dead_cell_fraction: spatial.star_dead_cell_fraction,
        star_uniformity: spatial.star_uniformity,
        bg_cell_spread: spatial.bg_cell_spread,
        bg_cell_max_dev: spatial.bg_cell_max_dev,
        median_adu: fits.stored_to_adu(stats.median),
        computed_at: chrono::Utc::now().timestamp(),
    })
}

/// Look up a cached entry that is still valid for the given filename.
pub fn valid_entry(
    store: &RwLock<SpatialMetricsStore>,
    image_id: i32,
    filename: &str,
) -> Option<StoredSpatialMetrics> {
    let s = store.read().unwrap();
    s.metrics
        .get(&image_id)
        .filter(|e| e.filename == filename)
        .cloned()
}

/// Snapshot of progress plus store size, for the progress endpoint.
pub fn progress_snapshot(store: &RwLock<SpatialMetricsStore>) -> (SpatialScanProgress, usize) {
    let s = store.read().unwrap();
    (s.progress.clone(), s.metrics.len())
}

pub type SharedSpatialStore = Arc<RwLock<SpatialMetricsStore>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn store_with(entries: Vec<StoredSpatialMetrics>) -> RwLock<SpatialMetricsStore> {
        RwLock::new(SpatialMetricsStore {
            metrics: entries.into_iter().map(|e| (e.image_id, e)).collect(),
            ..Default::default()
        })
    }

    fn entry(image_id: i32, filename: &str) -> StoredSpatialMetrics {
        StoredSpatialMetrics {
            image_id,
            filename: filename.to_string(),
            star_count: 4000,
            avg_hfr: 2.5,
            dead_cell_fraction: Some(0.1),
            star_uniformity: Some(0.7),
            bg_cell_spread: 0.05,
            bg_cell_max_dev: 0.04,
            median_adu: 1500.0,
            computed_at: 0,
        }
    }

    #[test]
    fn valid_entry_requires_matching_filename() {
        let store = store_with(vec![entry(1, "a.fits")]);
        assert!(valid_entry(&store, 1, "a.fits").is_some());
        assert!(valid_entry(&store, 1, "renamed.fits").is_none());
        assert!(valid_entry(&store, 2, "a.fits").is_none());
    }

    #[test]
    fn begin_scan_is_singleton() {
        let store = store_with(vec![]);
        assert!(try_begin_scan(&store, 5, None, 10, 2));
        assert!(
            !try_begin_scan(&store, 6, None, 3, 0),
            "second scan must be refused while one is running"
        );
        let (progress, _) = progress_snapshot(&store);
        assert_eq!(progress.target_id, Some(5));
        assert_eq!(progress.total, 10);
        assert_eq!(progress.skipped_cached, 2);
    }

    #[test]
    fn persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let store = store_with(vec![entry(1, "a.fits"), entry(2, "b.fits")]);
        persist(&store, dir.path());

        let fresh = RwLock::new(SpatialMetricsStore::default());
        ensure_loaded(&fresh, dir.path());
        assert!(valid_entry(&fresh, 1, "a.fits").is_some());
        assert!(valid_entry(&fresh, 2, "b.fits").is_some());
        // Loading is idempotent and tolerant of a missing file.
        ensure_loaded(&fresh, dir.path());
        let missing = RwLock::new(SpatialMetricsStore::default());
        ensure_loaded(&missing, Path::new("/nonexistent-dir-for-test"));
        assert_eq!(progress_snapshot(&missing).1, 0);
    }
}
