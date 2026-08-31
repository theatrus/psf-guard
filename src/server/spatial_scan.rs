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

use seiza_stretch::{stretch_u16_to_u16, StretchParams};
use serde::{Deserialize, Serialize};

use crate::image_analysis::FitsImage;
use crate::nina_star_detection::{
    detect_stars_with_original, NoiseReduction, StarDetectionParams, StarSensitivity,
};
use crate::photometry::{CatalogStar, FrameCatalog};
use crate::spatial_analysis::{compute_spatial_metrics, PixelCalibration, SpatialAnalysisConfig};

/// Persisted per-image spatial metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSpatialMetrics {
    pub image_id: i32,
    /// Basename of the FITS file the metrics were computed from; a changed
    /// filename invalidates the entry.
    pub filename: String,
    /// Durable upload revision, when this row is backed by remote-image
    /// provenance. Callers compare it with the live mapping before using the
    /// cached evidence.
    #[serde(default)]
    pub source_revision: Option<String>,
    /// Detector used for scheduler-compatible star count and HFR values.
    #[serde(default)]
    pub detector: String,
    /// Bump when detector inputs or measurement rules change.
    #[serde(default)]
    pub detector_version: u32,
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
    /// Dead-cell evidence expanded to the configured grid for overlays.
    #[serde(default)]
    pub star_dead_cells: Vec<bool>,
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
    /// Grid cells that contributed to `bg_glow_max`. Older cache entries do
    /// not contain this field; a later quality scan fills it.
    #[serde(default)]
    pub bg_glow_cells: Vec<bool>,
}

/// Stars kept per stored catalog: matching quality saturates well below full
/// catalog size, and this keeps spatial_metrics.json compact.
pub const STORED_CATALOG_STARS: usize = 300;

/// The Target Scheduler database records star count and HFR from N.I.N.A.'s
/// fast detector. Rescans must use the same detector family so sequence
/// baselines do not mix incompatible measurements.
pub const QUALITY_DETECTOR: &str = "nina_fast";
/// Bump when any cached pixel-quality input or measurement rule changes.
pub const QUALITY_DETECTOR_VERSION: u32 = 1;

/// Progress of the (singleton per-DB) spatial scan.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SpatialScanProgress {
    pub running: bool,
    /// `spatial`, `astrometry`, or `complete`.
    pub stage: String,
    pub target_id: Option<i32>,
    pub filter_name: Option<String>,
    pub total: usize,
    pub processed: usize,
    /// Images skipped because a cached entry already existed.
    pub skipped_cached: usize,
    #[serde(default)]
    pub spatial_processed: usize,
    #[serde(default)]
    pub astrometry_processed: usize,
    #[serde(default)]
    pub solved: usize,
    #[serde(default)]
    pub solve_failed: usize,
    #[serde(default)]
    pub operational_errors: usize,
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
    /// Incremented when the source behind one scheduler row changes. A scan
    /// that began before an upload remap may finish, but cannot publish
    /// measurements from the old pixels afterward.
    source_generations: HashMap<i32, u64>,
}

/// One unit of scan work, resolved from the DB before the blocking task runs.
#[derive(Debug, Clone)]
pub struct ScanWorkItem {
    pub image_id: i32,
    pub filename: String,
    pub fits_path: PathBuf,
    pub source_generation: u64,
    pub source_revision: Option<String>,
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

pub fn source_generation(store: &RwLock<SpatialMetricsStore>, image_id: i32) -> u64 {
    store
        .read()
        .unwrap()
        .source_generations
        .get(&image_id)
        .copied()
        .unwrap_or(0)
}

/// Forget pixel evidence for a row whose durable upload source changed and
/// invalidate any scan work that resolved the old source before the remap.
pub fn invalidate_image_source(
    store: &RwLock<SpatialMetricsStore>,
    cache_dir: &Path,
    image_id: i32,
) -> bool {
    ensure_loaded(store, cache_dir);
    {
        let mut state = store.write().unwrap();
        let generation = state.source_generations.entry(image_id).or_default();
        *generation = generation.wrapping_add(1);
        state.metrics.remove(&image_id).is_some()
    }
}

fn record_scan_entry_if_current(
    store: &RwLock<SpatialMetricsStore>,
    item: &ScanWorkItem,
    entry: StoredSpatialMetrics,
) -> bool {
    let mut state = store.write().unwrap();
    let current = state
        .source_generations
        .get(&item.image_id)
        .copied()
        .unwrap_or(0);
    if current != item.source_generation {
        return false;
    }
    state.metrics.insert(item.image_id, entry);
    true
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
        stage: "spatial".to_string(),
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
    wait_for_turn: &(dyn Fn() + Sync),
) {
    use std::sync::atomic::{AtomicUsize, Ordering};

    let spatial_config = SpatialAnalysisConfig::default();
    let since_persist = AtomicUsize::new(0);

    // Shared work-stealing pool sized by the caller's worker budget.
    crate::concurrency::parallel_index(work.len(), workers, |i| {
        wait_for_turn();
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
                let recorded = record_scan_entry_if_current(store, item, entry);
                let mut s = store.write().unwrap();
                s.progress.processed += 1;
                s.progress.spatial_processed += 1;
                if !recorded {
                    tracing::info!(
                        image_id = item.image_id,
                        "Discarded quality metrics because the image source changed during the scan"
                    );
                }
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
                s.progress.spatial_processed += 1;
                s.progress.last_error = Some(format!("{}: {}", item.filename, e));
            }
        }

        if since_persist.fetch_add(1, Ordering::Relaxed) + 1 >= PERSIST_EVERY {
            since_persist.store(0, Ordering::Relaxed);
            persist(store, cache_dir);
        }
    });

    persist(store, cache_dir);
}

pub fn begin_astrometry_stage(store: &RwLock<SpatialMetricsStore>, total: usize) {
    let mut s = store.write().unwrap();
    s.progress.stage = "astrometry".to_string();
    s.progress.total = total;
    s.progress.processed = 0;
    s.progress.current_file = None;
}

/// Publish the file about to be solved so progress polling shows the frame
/// currently occupying the (multi-second) solver, not the last finished one.
pub fn begin_astrometry_item(store: &RwLock<SpatialMetricsStore>, filename: &str) {
    let mut s = store.write().unwrap();
    s.progress.current_file = Some(filename.to_string());
}

pub fn record_astrometry_result(
    store: &RwLock<SpatialMetricsStore>,
    filename: &str,
    solved: bool,
    quality_failure: bool,
    operational_error: Option<String>,
) {
    let mut s = store.write().unwrap();
    s.progress.processed += 1;
    s.progress.astrometry_processed += 1;
    if solved {
        s.progress.solved += 1;
    } else if quality_failure {
        s.progress.solve_failed += 1;
    }
    if let Some(error) = operational_error {
        s.progress.errors += 1;
        s.progress.operational_errors += 1;
        s.progress.last_error = Some(format!("{filename}: {error}"));
    }
}

/// Mark the scan finished. Split out so callers can guarantee finalization
/// even when the scan body fails unexpectedly.
pub fn finalize_scan(store: &RwLock<SpatialMetricsStore>) {
    let mut s = store.write().unwrap();
    s.progress.running = false;
    s.progress.stage = "complete".to_string();
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

    let params = StarDetectionParams {
        sensitivity: StarSensitivity::Normal,
        noise_reduction: NoiseReduction::None,
        use_roi: false,
    };
    let stretch_params = StretchParams::default();
    let stretched = stretch_u16_to_u16(&fits.data, &stats.to_stretch_statistics(), &stretch_params);
    let result =
        detect_stars_with_original(&stretched, &fits.data, fits.width, fits.height, &params);
    let positions: Vec<(f64, f64)> = result.star_list.iter().map(|s| s.position).collect();
    // N.I.N.A. measures each accepted star on the full-resolution original.
    // Convert its background-subtracted aperture flux from stored units to
    // physical ADU for cross-frame photometry.
    let catalog = FrameCatalog {
        stars: result
            .star_list
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
        source_revision: item.source_revision.clone(),
        detector: QUALITY_DETECTOR.to_string(),
        detector_version: QUALITY_DETECTOR_VERSION,
        star_count: result.star_list.len(),
        avg_hfr: result.average_hfr,
        dead_cell_fraction: spatial.star_dead_cell_fraction,
        star_uniformity: spatial.star_uniformity,
        bg_cell_spread: spatial.bg_cell_spread,
        bg_cell_max_dev: spatial.bg_cell_max_dev,
        median_adu: fits.stored_to_adu(stats.median),
        computed_at: chrono::Utc::now().timestamp(),
        catalog,
        star_cell_counts: spatial.star_cell_counts,
        star_dead_cells: spatial.star_dead_cells,
        bg_cell_medians: spatial.bg_cell_medians,
        grid_cols: config.grid_cols,
        grid_rows: config.grid_rows,
        width: fits.width,
        height: fits.height,
        exposure_s: headers.exposure_s,
        bg_glow_max: spatial.bg_glow_max,
        bg_glow_cells: spatial.bg_glow_cells,
    })
}

/// Whether a metadata JSON is missing star metrics a quality scan can fill.
///
/// Header-first imports omit `DetectedStars` and `HFR` because no pixel
/// evidence exists at import time. Unparsable metadata answers false: there
/// is nothing safe to fill.
pub fn metadata_lacks_star_metrics(metadata_json: &str) -> bool {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(metadata_json) else {
        return false;
    };
    let Some(map) = value.as_object() else {
        return false;
    };
    let missing = |key: &str| map.get(key).is_none_or(serde_json::Value::is_null);
    missing("DetectedStars") || missing("HFR")
}

/// Fill scan-measured star metrics into a metadata JSON that lacks them.
///
/// The scan's detector is the scheduler-compatible one (`nina_fast`), so the
/// filled values mean the same thing as a N.I.N.A. catalog's. Existing values
/// are never overwritten, and a frame with no detected stars gets no HFR:
/// zero would read as an impossibly sharp measurement rather than "none".
/// Returns `None` when nothing was added.
pub fn star_metrics_metadata_patch(
    metadata_json: &str,
    star_count: usize,
    avg_hfr: f64,
    source_revision: Option<&str>,
) -> Option<String> {
    let mut value: serde_json::Value = serde_json::from_str(metadata_json).ok()?;
    let map = value.as_object_mut()?;
    let missing = |map: &serde_json::Map<String, serde_json::Value>, key: &str| {
        map.get(key).is_none_or(serde_json::Value::is_null)
    };
    let same_source = source_revision.is_some_and(|source_revision| {
        map.iter().any(|(key, value)| {
            key.eq_ignore_ascii_case("PsfGuardQualitySource")
                && value.as_str() == Some(source_revision)
        })
    });
    let mut supplied = if same_source {
        map.iter()
            .find_map(|(key, value)| {
                key.eq_ignore_ascii_case("PsfGuardQualityFields")
                    .then(|| value.as_array())
                    .flatten()
            })
            .map(|fields| {
                fields
                    .iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let supplied_before = supplied.len();
    if missing(map, "DetectedStars") {
        map.insert("DetectedStars".to_string(), (star_count as u64).into());
        supplied.push("DetectedStars".to_string());
    }
    if star_count > 0 && avg_hfr > 0.0 && missing(map, "HFR") {
        map.insert("HFR".to_string(), avg_hfr.into());
        supplied.push("HFR".to_string());
    }
    let changed = supplied.len() != supplied_before;
    if changed && let Some(source_revision) = source_revision {
        map.insert("PsfGuardQualitySource".to_string(), source_revision.into());
        map.insert("PsfGuardQualityFields".to_string(), supplied.clone().into());
    }
    changed.then(|| value.to_string())
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

/// Look up an entry whose pixel provenance still agrees with the durable
/// remote-image mapping. Native scheduler files have no mapping revision; in
/// that case a previously mapped entry must not be reused.
pub fn valid_entry_for_source(
    store: &RwLock<SpatialMetricsStore>,
    image_id: i32,
    filename: &str,
    mapped_source_revision: Option<&str>,
) -> Option<StoredSpatialMetrics> {
    valid_entry(store, image_id, filename).filter(|entry| match mapped_source_revision {
        Some(expected) => entry.source_revision.as_deref() == Some(expected),
        None => !entry
            .source_revision
            .as_deref()
            .is_some_and(|revision| revision.starts_with("mapping:")),
    })
}

/// Look up an entry that contains every field produced by the current quality
/// scan. Older cache files deserialize successfully, but their defaulted grid
/// dimensions and cell arrays cannot support photometric screening.
pub fn valid_quality_entry(
    store: &RwLock<SpatialMetricsStore>,
    image_id: i32,
    filename: &str,
) -> Option<StoredSpatialMetrics> {
    valid_entry(store, image_id, filename).filter(|entry| quality_entry_is_current(entry, filename))
}

pub fn valid_quality_entry_for_source(
    store: &RwLock<SpatialMetricsStore>,
    image_id: i32,
    filename: &str,
    mapped_source_revision: Option<&str>,
) -> Option<StoredSpatialMetrics> {
    valid_entry_for_source(store, image_id, filename, mapped_source_revision)
        .filter(|entry| quality_entry_is_current(entry, filename))
}

/// Whether a cached entry matches the source and current quality model.
pub fn quality_entry_is_current(entry: &StoredSpatialMetrics, filename: &str) -> bool {
    let cells = entry.grid_cols.saturating_mul(entry.grid_rows);
    entry.filename == filename
        && entry.detector == QUALITY_DETECTOR
        && entry.detector_version == QUALITY_DETECTOR_VERSION
        && entry.width > 0
        && entry.height > 0
        && cells > 0
        && entry.star_cell_counts.len() == cells
        && entry.bg_cell_medians.len() == cells
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
            source_revision: None,
            detector: String::new(),
            detector_version: 0,
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
            star_dead_cells: vec![],
            bg_cell_medians: vec![],
            grid_cols: 8,
            grid_rows: 6,
            width: 0,
            height: 0,
            exposure_s: None,
            bg_glow_max: 0.0,
            bg_glow_cells: vec![],
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
    fn complete_quality_entry_requires_current_photometry_inputs() {
        let legacy = entry(1, "legacy.fits");
        let mut current = entry(2, "current.fits");
        current.detector = QUALITY_DETECTOR.to_string();
        current.detector_version = QUALITY_DETECTOR_VERSION;
        current.width = 6248;
        current.height = 4176;
        current.star_cell_counts = vec![0.0; 48];
        current.bg_cell_medians = vec![1000.0; 48];
        let mut dimensions_only = entry(3, "dimensions-only.fits");
        dimensions_only.detector = QUALITY_DETECTOR.to_string();
        dimensions_only.detector_version = QUALITY_DETECTOR_VERSION;
        dimensions_only.width = 6248;
        dimensions_only.height = 4176;
        let store = store_with(vec![legacy, current, dimensions_only]);

        assert!(valid_quality_entry(&store, 1, "legacy.fits").is_none());
        assert!(valid_quality_entry(&store, 2, "current.fits").is_some());
        assert!(valid_quality_entry(&store, 3, "dimensions-only.fits").is_none());
    }

    #[test]
    fn complete_quality_entry_rejects_other_detector_versions() {
        let mut old = entry(1, "old.fits");
        old.detector = QUALITY_DETECTOR.to_string();
        old.detector_version = QUALITY_DETECTOR_VERSION.saturating_sub(1);
        old.width = 6248;
        old.height = 4176;
        old.star_cell_counts = vec![0.0; 48];
        old.bg_cell_medians = vec![1000.0; 48];

        let store = store_with(vec![old]);
        assert!(valid_quality_entry(&store, 1, "old.fits").is_none());
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
    fn metadata_star_metrics_fill_only_missing_keys() {
        // Header-first import: both keys absent → both filled.
        let imported = r#"{"FileName":"a.xisf","SessionId":0}"#;
        assert!(metadata_lacks_star_metrics(imported));
        let patched =
            star_metrics_metadata_patch(imported, 120, 2.5, Some("file:source-a")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(value["DetectedStars"], 120);
        assert_eq!(value["HFR"], 2.5);
        assert_eq!(value["PsfGuardQualitySource"], "file:source-a");
        assert_eq!(
            value["PsfGuardQualityFields"],
            serde_json::json!(["DetectedStars", "HFR"])
        );
        assert_eq!(value["FileName"], "a.xisf", "existing keys must survive");

        // N.I.N.A. catalog: measurements present → untouched.
        let nina = r#"{"DetectedStars":300,"HFR":1.8}"#;
        assert!(!metadata_lacks_star_metrics(nina));
        assert!(star_metrics_metadata_patch(nina, 120, 2.5, None).is_none());

        // Null counts as missing (a writer may serialize unknowns as null).
        let with_null = r#"{"DetectedStars":null,"HFR":1.8}"#;
        assert!(metadata_lacks_star_metrics(with_null));
        let patched = star_metrics_metadata_patch(with_null, 120, 2.5, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(value["DetectedStars"], 120);
        assert_eq!(value["HFR"], 1.8, "measured HFR must not be overwritten");

        let patched = star_metrics_metadata_patch(
            r#"{"DetectedStars":300}"#,
            120,
            2.5,
            Some("mapping:current"),
        )
        .unwrap();
        let value: serde_json::Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(value["DetectedStars"], 300);
        assert_eq!(value["PsfGuardQualityFields"], serde_json::json!(["HFR"]));

        let first = star_metrics_metadata_patch("{}", 0, 0.0, Some("mapping:current")).unwrap();
        let second = star_metrics_metadata_patch(&first, 20, 2.0, Some("mapping:current")).unwrap();
        let value: serde_json::Value = serde_json::from_str(&second).unwrap();
        assert_eq!(
            value["PsfGuardQualityFields"],
            serde_json::json!(["DetectedStars", "HFR"])
        );
    }

    #[test]
    fn metadata_star_metrics_fill_handles_edge_inputs() {
        // No detected stars: the count is a real measurement, HFR is not.
        let patched = star_metrics_metadata_patch("{}", 0, 0.0, None).unwrap();
        let value: serde_json::Value = serde_json::from_str(&patched).unwrap();
        assert_eq!(value["DetectedStars"], 0);
        assert!(value.get("HFR").is_none(), "no stars → no HFR measurement");

        // Unparsable or non-object metadata: nothing to check, nothing to fill.
        assert!(!metadata_lacks_star_metrics("not json"));
        assert!(star_metrics_metadata_patch("not json", 10, 2.0, None).is_none());
        assert!(!metadata_lacks_star_metrics("[1,2]"));
        assert!(star_metrics_metadata_patch("[1,2]", 10, 2.0, None).is_none());
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

    #[test]
    fn source_invalidation_removes_metrics_and_discards_in_flight_results() {
        let directory = tempfile::tempdir().unwrap();
        let mut old_entry = entry(7, "old.fits");
        old_entry.source_revision = Some("mapping-old".into());
        let store = store_with(vec![old_entry.clone()]);
        let item = ScanWorkItem {
            image_id: 7,
            filename: "old.fits".into(),
            fits_path: directory.path().join("old.fits"),
            source_generation: source_generation(&store, 7),
            source_revision: Some("mapping-old".into()),
        };

        assert!(invalidate_image_source(&store, directory.path(), 7));
        assert!(!store.read().unwrap().metrics.contains_key(&7));
        assert!(!record_scan_entry_if_current(
            &store,
            &item,
            old_entry.clone()
        ));
        assert_eq!(source_generation(&store, 7), 1);
        persist(&store_with(vec![old_entry]), directory.path());
        let restarted = RwLock::new(SpatialMetricsStore::default());
        ensure_loaded(&restarted, directory.path());
        assert!(
            valid_quality_entry_for_source(&restarted, 7, "old.fits", Some("mapping-new"))
                .is_none()
        );

        let mut current_entry = entry(7, "old.fits");
        current_entry.source_revision = Some("mapping-new".into());
        let current_item = ScanWorkItem {
            source_generation: source_generation(&store, 7),
            source_revision: Some("mapping-new".into()),
            ..item
        };
        assert!(record_scan_entry_if_current(
            &store,
            &current_item,
            current_entry
        ));
        persist(&store, directory.path());
        let restarted = RwLock::new(SpatialMetricsStore::default());
        ensure_loaded(&restarted, directory.path());
        assert!(restarted.read().unwrap().metrics.contains_key(&7));
    }
}
