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
use crate::photometry::{CatalogStar, FrameCatalog};
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
    /// Brightest detected stars (positions + ADU flux) for cross-frame
    /// photometry. Empty on entries computed before photometric screening
    /// existed; a re-scan fills them.
    #[serde(default)]
    pub catalog: FrameCatalog,
    /// Star counts per cell at the configured grid (row-major).
    #[serde(default)]
    pub star_cell_counts: Vec<f64>,
    /// Background medians per cell in ADU (row-major).
    #[serde(default)]
    pub bg_cell_medians: Vec<f64>,
    #[serde(default)]
    pub grid_cols: usize,
    #[serde(default)]
    pub grid_rows: usize,
    #[serde(default)]
    pub width: usize,
    #[serde(default)]
    pub height: usize,
    /// Exposure seconds from the FITS header (photometry groups by exposure).
    #[serde(default)]
    pub exposure_s: Option<f64>,
    /// Static within-frame glow (max positive robust-plane residual as a
    /// fraction of sky).
    #[serde(default)]
    pub bg_glow_max: f64,
}

/// Stars kept per stored catalog: matching quality saturates well below full
/// catalog size, and this keeps spatial_metrics.json compact.
pub const STORED_CATALOG_STARS: usize = 300;

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
    use std::sync::atomic::{AtomicU64, Ordering};
    // Unique temp file per call: two scan workers can persist concurrently,
    // and a shared temp path would let one rename publish the other's
    // partially written file. Renames of distinct complete files are atomic;
    // last writer wins.
    static PERSIST_SEQ: AtomicU64 = AtomicU64::new(0);

    let entries: Vec<StoredSpatialMetrics> = {
        let s = store.read().unwrap();
        s.metrics.values().cloned().collect()
    };
    let path = persist_path(cache_dir);
    let tmp = path.with_extension(format!(
        "json.tmp.{}.{}",
        std::process::id(),
        PERSIST_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
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
        let _ = std::fs::remove_file(&tmp);
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

/// Run the scan synchronously (call from `spawn_blocking`). `work` must only
/// contain images that actually need computing. `workers` is the desired
/// concurrency (see `concurrency::plan_workers`), clamped here to the
/// amount of work. Detection is CPU-bound at several seconds per full-frame
/// image, so each worker roughly adds one frame's worth of throughput.
pub fn run_scan(
    store: &RwLock<SpatialMetricsStore>,
    cache_dir: &Path,
    work: &[ScanWorkItem],
    workers: usize,
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let spatial_config = SpatialAnalysisConfig::default();
    let since_persist = AtomicUsize::new(0);

    // Shared work-stealing pool sized by the caller's worker budget.
    crate::concurrency::parallel_index(work.len(), workers, |i| {
        let item = &work[i];
        {
            let mut s = store.write().unwrap();
            s.progress.current_file = Some(item.filename.clone());
        }

        // A panic here (malformed FITS tripping an assert deep in detection)
        // must not escape: it would propagate through the pool's thread::scope,
        // skip the finalization below, and leave the per-DB scan singleton
        // wedged at running=true until restart. compute_one holds no store
        // lock, so catching cannot poison.
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            compute_one(item, &spatial_config)
        }))
        .unwrap_or_else(|panic| {
            let msg = panic
                .downcast_ref::<&str>()
                .map(|s| s.to_string())
                .or_else(|| panic.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panic during analysis".to_string());
            Err(anyhow::anyhow!("panicked: {}", msg))
        });

        match outcome {
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

    persist(store, cache_dir);
    finalize_scan(store);
}

/// Mark the scan finished. Split out so callers can guarantee finalization
/// even when the scan body fails unexpectedly.
pub fn finalize_scan(store: &RwLock<SpatialMetricsStore>) {
    let mut s = store.write().unwrap();
    s.progress.running = false;
    s.progress.current_file = None;
    s.progress.finished_at = Some(chrono::Utc::now().timestamp());
}

fn compute_one(
    item: &ScanWorkItem,
    config: &SpatialAnalysisConfig,
) -> anyhow::Result<StoredSpatialMetrics> {
    let headers = crate::commands::screen_fits::extract_headers(&item.fits_path);
    let fits = FitsImage::from_file(&item.fits_path)?;
    let stats = fits.calculate_basic_statistics();

    let params = HocusFocusParams::default();
    let result = detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);
    let positions: Vec<(f64, f64)> = result.stars.iter().map(|s| s.position).collect();
    // Background-subtracted fluxes in stored units are linear in raw_scale;
    // divide for cross-frame-comparable ADU.
    let catalog = FrameCatalog {
        stars: result
            .stars
            .iter()
            .filter(|s| s.flux > 0.0)
            .map(|s| CatalogStar {
                x: s.position.0,
                y: s.position.1,
                flux: s.flux / fits.raw_scale,
            })
            .collect(),
    }
    .truncated(STORED_CATALOG_STARS);

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
        catalog,
        star_cell_counts: spatial.star_cell_counts,
        bg_cell_medians: spatial.bg_cell_medians,
        grid_cols: config.grid_cols,
        grid_rows: config.grid_rows,
        width: fits.width,
        height: fits.height,
        exposure_s: headers.exposure_s,
        bg_glow_max: spatial.bg_glow_max,
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
            catalog: crate::photometry::FrameCatalog::default(),
            star_cell_counts: vec![],
            bg_cell_medians: vec![],
            grid_cols: 8,
            grid_rows: 6,
            width: 0,
            height: 0,
            exposure_s: None,
            bg_glow_max: 0.0,
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
