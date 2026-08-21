//! Bounded interactive queue for on-demand preview / annotated PNG generation.
//!
//! Previously the preview and annotated handlers generated a missing PNG
//! *inside the request* (a multi-second `spawn_blocking`), so the browser's
//! `<img>` GET stayed pending the whole time and nothing bounded how many ran
//! at once. This queue moves that work off the request: on a cache miss the
//! handler enqueues a job and returns immediately (HTTP 202), and this pool
//! generates the PNG with bounded, interactive-priority concurrency.
//!
//! It reuses PR #149's machinery: the pool is sized by
//! [`crate::concurrency::plan_workers`] at [`Priority::Interactive`] (memory-
//! bounded via a frame probe), and every job holds an
//! [`AppState::begin_interactive_job`] guard for its lifetime, so background
//! pre-generation yields cores + memory to user-driven preview work.
//!
//! Because readiness is now observed by a *different* request (via
//! `Path::exists`), generation writes to a temp file and atomically renames,
//! so a poll never sees a half-written PNG.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use serde::Serialize;
use tokio::sync::Semaphore;

use crate::concurrency::{self, Priority, WorkerPolicy};
use crate::server::state::AppState;

/// What to generate for a job. Mirrors the two artifact handlers.
#[derive(Debug, Clone)]
pub enum GenKind {
    Preview {
        midtone: f64,
        shadow: f64,
        max_dimensions: Option<(u32, u32)>,
        /// Render a one-shot-color mosaic in colour rather than luminance.
        /// A frame with no `BAYERPAT` falls back to greyscale, so this can be
        /// asked for on a mixed rig without breaking the mono frames.
        color: bool,
    },
    Annotated {
        max_stars: usize,
        size: String,
    },
}

/// A resolved generation request: where the source is, where the artifact goes.
#[derive(Debug, Clone)]
pub struct GenJob {
    pub fits_path: PathBuf,
    pub cache_path: PathBuf,
    pub kind: GenKind,
    /// Carried on the job rather than read from state at write time, so the
    /// bytes written always match the extension the path was chosen for.
    pub encoding: crate::preview_format::PreviewEncoding,
}

/// Readiness of a cache artifact, as reported to the polling frontend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum GenerationState {
    Ready,
    Generating,
    Error,
}

/// Status payload for one artifact (batch-status response element).
#[derive(Debug, Clone, Serialize)]
pub struct GenerationStatus {
    pub state: GenerationState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl GenerationStatus {
    fn ready() -> Self {
        Self {
            state: GenerationState::Ready,
            error: None,
        }
    }
    fn generating() -> Self {
        Self {
            state: GenerationState::Generating,
            error: None,
        }
    }
    fn error(msg: String) -> Self {
        Self {
            state: GenerationState::Error,
            error: Some(msg),
        }
    }
}

/// Recent-error map cap, so a run of unresolvable frames can't grow it forever.
const MAX_RECENT_ERRORS: usize = 512;
const GENERATION_ERROR_MESSAGE: &str =
    "preview generation failed; it will retry when the source file changes";

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFingerprint {
    canonical_path: PathBuf,
    len: u64,
    modified: Option<SystemTime>,
}

fn source_fingerprint(path: &Path) -> Option<SourceFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(SourceFingerprint {
        canonical_path: std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf()),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    })
}

#[derive(Debug, Clone)]
struct RecentError {
    source_fingerprint: Option<SourceFingerprint>,
}

#[derive(Default)]
struct QueueInner {
    /// `cache_path`s currently being generated (dedup).
    in_flight: HashSet<PathBuf>,
    /// `cache_path` -> fingerprint of the source that failed. Error text is
    /// deliberately not retained for API responses because decoder messages
    /// can include a server-local path.
    recent_errors: HashMap<PathBuf, RecentError>,
    /// Source identity that produced a completed artifact in this process.
    /// Tracked remote arrivals use it to invalidate a preview if the copy
    /// resumes after an apparently stable prefix was rendered.
    completed_sources: HashMap<PathBuf, SourceFingerprint>,
}

/// Process-global interactive preview/annotated generation queue. Held on
/// [`AppState`]; the actual dispatch lives in [`AppState::enqueue_preview`] so
/// it can take an interactive-job guard from the same `AppState`.
#[derive(Default)]
pub struct PreviewQueue {
    inner: Mutex<QueueInner>,
    /// Sized lazily on first job (needs a real frame to probe for the memory
    /// ceiling); reused thereafter.
    semaphore: Mutex<Option<Arc<Semaphore>>>,
}

impl PreviewQueue {
    pub fn cached_source_matches(&self, cache_path: &Path, source_path: &Path) -> bool {
        let current = source_fingerprint(source_path);
        let inner = self.inner.lock().unwrap();
        inner.completed_sources.get(cache_path) == current.as_ref()
    }

    pub fn forget_completed_source(&self, cache_path: &Path) {
        self.inner
            .lock()
            .unwrap()
            .completed_sources
            .remove(cache_path);
    }

    #[cfg(test)]
    pub(crate) fn remember_completed_source_for_test(
        &self,
        cache_path: PathBuf,
        source_path: &Path,
    ) {
        self.inner.lock().unwrap().completed_sources.insert(
            cache_path,
            source_fingerprint(source_path).expect("test source fingerprint"),
        );
    }

    /// Report the state of one artifact by its `cache_path`. Pure read — does
    /// not enqueue (the caller enqueues when appropriate).
    pub fn status(&self, cache_path: &Path) -> Option<GenerationStatus> {
        // A completed artifact is always the truth, even if a stale error entry
        // lingers.
        if cache_path.exists() {
            return Some(GenerationStatus::ready());
        }
        let inner = self.inner.lock().unwrap();
        if inner.in_flight.contains(cache_path) {
            return Some(GenerationStatus::generating());
        }
        inner
            .recent_errors
            .contains_key(cache_path)
            .then(|| GenerationStatus::error(GENERATION_ERROR_MESSAGE.to_string()))
    }

    /// Report status for a resolved source. A previous failure is forgotten
    /// once that source's size or modification time changes, allowing a file
    /// copied in place to resume preview generation without a server restart.
    pub fn status_for_source(
        &self,
        cache_path: &Path,
        source_path: &Path,
    ) -> Option<GenerationStatus> {
        if cache_path.exists() {
            return Some(GenerationStatus::ready());
        }
        let fingerprint = source_fingerprint(source_path);
        let mut inner = self.inner.lock().unwrap();
        if inner.in_flight.contains(cache_path) {
            return Some(GenerationStatus::generating());
        }
        match inner.recent_errors.get(cache_path) {
            Some(error) if error.source_fingerprint.as_ref() == fingerprint.as_ref() => Some(
                GenerationStatus::error(GENERATION_ERROR_MESSAGE.to_string()),
            ),
            Some(_) => {
                inner.recent_errors.remove(cache_path);
                None
            }
            None => None,
        }
    }

    /// Lazily create (and reuse) the concurrency-bounding semaphore, sized from
    /// a representative frame so a big sensor on a high-core box can't OOM.
    fn semaphore(&self, policy: &WorkerPolicy, sample_fits: &Path) -> Arc<Semaphore> {
        let mut slot = self.semaphore.lock().unwrap();
        if let Some(s) = &*slot {
            return Arc::clone(s);
        }
        let frame_pixels = concurrency::probe_frame_pixels(sample_fits);
        let budget = concurrency::plan_workers(None, policy, Priority::Interactive, frame_pixels);
        tracing::info!(
            "🖼️ Preview generation pool: {} worker(s) — {}",
            budget.workers,
            budget.rationale
        );
        let s = Arc::new(Semaphore::new(budget.workers.max(1)));
        *slot = Some(Arc::clone(&s));
        s
    }
}

impl AppState {
    /// Enqueue a preview/annotated generation job on the bounded interactive
    /// pool. Idempotent: a `cache_path` already present or already in-flight is
    /// a no-op, so the same artifact is never generated twice concurrently and
    /// re-requests are cheap.
    pub fn enqueue_preview(self: &Arc<Self>, job: GenJob) {
        let current_source = source_fingerprint(&job.fits_path);
        // Dedup + claim the slot under the lock. `insert` returns false when
        // the path was already in-flight.
        {
            let mut inner = self.preview_queue.inner.lock().unwrap();
            if job.cache_path.exists()
                || inner
                    .recent_errors
                    .get(&job.cache_path)
                    .is_some_and(|error| {
                        error.source_fingerprint.as_ref() == current_source.as_ref()
                    })
                || !inner.in_flight.insert(job.cache_path.clone())
            {
                return;
            }
            inner.recent_errors.remove(&job.cache_path);
        }

        let sem = self
            .preview_queue
            .semaphore(&self.worker_policy(), &job.fits_path);
        let state = Arc::clone(self);
        tokio::spawn(async move {
            // Mark interactive-active for the whole job so background pregen
            // yields; drops even if the task is cancelled/panics.
            let _guard = state.begin_interactive_job();
            // Bound concurrency to the interactive budget. A closed semaphore
            // (never happens — we never close it) would just skip the permit.
            let _permit = sem.acquire_owned().await;

            let cache_path = job.cache_path.clone();
            let attempted_source = source_fingerprint(&job.fits_path);
            let worker_source = attempted_source.clone();
            let outcome =
                tokio::task::spawn_blocking(move || generate_with_fingerprint(&job, worker_source))
                    .await;

            let mut inner = state.preview_queue.inner.lock().unwrap();
            inner.in_flight.remove(&cache_path);
            match outcome {
                Ok(Ok(())) => {
                    inner.recent_errors.remove(&cache_path);
                    if let Some(source) = attempted_source {
                        if inner.completed_sources.len() >= MAX_RECENT_ERRORS * 16
                            && let Some(oldest) = inner.completed_sources.keys().next().cloned()
                        {
                            inner.completed_sources.remove(&oldest);
                        }
                        inner.completed_sources.insert(cache_path, source);
                    }
                }
                Ok(Err(error)) => {
                    inner.completed_sources.remove(&cache_path);
                    record_error(&mut inner, cache_path, attempted_source, error.to_string())
                }
                Err(join) => record_error(
                    &mut inner,
                    cache_path,
                    attempted_source,
                    format!("panicked: {join}"),
                ),
            }
        });
    }
}

fn record_error(
    inner: &mut QueueInner,
    cache_path: PathBuf,
    source_fingerprint: Option<SourceFingerprint>,
    detail: String,
) {
    tracing::warn!(
        "🖼️ Preview generation failed for {}: {}",
        cache_path.display(),
        detail
    );
    if inner.recent_errors.len() >= MAX_RECENT_ERRORS
        && let Some(oldest) = inner.recent_errors.keys().next().cloned()
    {
        inner.recent_errors.remove(&oldest);
    }
    inner
        .recent_errors
        .insert(cache_path, RecentError { source_fingerprint });
}

/// Generate one artifact to a unique temp path, then atomically rename into
/// place, so a concurrent `Path::exists` poll never observes a partial file.
/// Shared by the on-demand queue and the background pre-generation task, so
/// both are atomic and a pregen/queue double-generate can't clobber a reader.
/// Blocking; call from `spawn_blocking`.
pub fn generate(job: &GenJob) -> anyhow::Result<()> {
    let attempted_source = source_fingerprint(&job.fits_path);
    generate_with_fingerprint(job, attempted_source)
}

fn generate_with_fingerprint(
    job: &GenJob,
    attempted_source: Option<SourceFingerprint>,
) -> anyhow::Result<()> {
    let tmp = temp_path(&job.cache_path);
    let result = match &job.kind {
        GenKind::Preview {
            midtone,
            shadow,
            max_dimensions,
            color,
        } => generate_preview(
            &job.fits_path,
            &tmp,
            *midtone,
            *shadow,
            *max_dimensions,
            *color,
            // Neither is reachable from the API today. Passed through rather
            // than hardcoded so that when one is, it arrives at a renderer
            // that honours it instead of being dropped here.
            false, // logarithmic
            false, // invert
            job.encoding,
        ),
        GenKind::Annotated { max_stars, size } => {
            generate_annotated(&job.fits_path, &tmp, size, *max_stars, job.encoding)
        }
    };

    // Clean up the temp file on both a generation failure and a rename
    // failure, so a failed run never orphans a `.tmp.*` file.
    let result = result.and_then(|()| {
        anyhow::ensure!(
            source_fingerprint(&job.fits_path).as_ref() == attempted_source.as_ref(),
            "source file changed while the preview was generated"
        );
        std::fs::rename(&tmp, &job.cache_path).map_err(Into::into)
    });
    match result {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Render one preview, in colour when asked for it and the frame is a mosaic.
#[allow(clippy::too_many_arguments)]
fn generate_preview(
    fits_path: &Path,
    output: &Path,
    midtone: f64,
    shadow: f64,
    max_dimensions: Option<(u32, u32)>,
    color: bool,
    logarithmic: bool,
    invert: bool,
    encoding: crate::preview_format::PreviewEncoding,
) -> anyhow::Result<()> {
    let source = fits_path.to_string_lossy();
    let destination = output.to_string_lossy().into_owned();
    if color
        && !logarithmic
        && !invert
        && crate::commands::stretch_to_png::render_color_preview(
            &source,
            Some(destination.clone()),
            midtone,
            shadow,
            max_dimensions,
            encoding,
        )?
    {
        return Ok(());
    }
    crate::commands::stretch_to_png::render_preview(
        &source,
        Some(destination),
        midtone,
        shadow,
        logarithmic,
        invert,
        max_dimensions,
        encoding,
    )
}

/// Unique sibling temp path for atomic-rename generation.
pub fn temp_path(cache_path: &Path) -> PathBuf {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let mut s = cache_path.as_os_str().to_os_string();
    s.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    PathBuf::from(s)
}

/// Build the annotated (star-marked) PNG for a frame and write it to `out_path`.
/// Extracted from the former inline handler body so the queue worker and the
/// pre-generation task share one implementation.
pub fn generate_annotated(
    fits_path: &Path,
    out_path: &Path,
    size: &str,
    max_stars: usize,
    encoding: crate::preview_format::PreviewEncoding,
) -> anyhow::Result<()> {
    use crate::commands::annotate_stars_common::create_annotated_image;
    use crate::image_analysis::FitsImage;
    use image::{ColorType, Rgb};

    let fits = FitsImage::from_file(fits_path)?;
    // Telescope-class preset from the frame's own headers, so a wide-field
    // or long-focal-length frame is annotated with knobs sized to its stars.
    let (params, _class) =
        crate::hocus_focus_star_detection::HocusFocusParams::for_frame_path(fits_path);
    let label_scale = hfr_label_scale_for(fits.width as u32, size);
    let rgb = create_annotated_image(
        &fits,
        &params,
        max_stars,
        0.2,
        -2.8,
        Rgb([255, 255, 0]),
        Some(label_scale),
    )?;
    let final_image = resize_rgb_for_size(rgb, fits.width, fits.height, size);

    let (w, h) = final_image.dimensions();
    // The markers are line art, where JPEG rings hardest. An operator who
    // chose JPEG for the cache gets it here too rather than a surprising
    // exception, but that is the place to look first if a marker seems to
    // have a halo.
    encoding.write(out_path, &final_image, w, h, ColorType::Rgb8)
}

/// Resize an RGB image to the requested size bucket (matches the preview
/// dimension buckets): `large` → 2000px, `original` → none, else → 1200px.
fn resize_rgb_for_size(
    img: image::RgbImage,
    width: usize,
    height: usize,
    size: &str,
) -> image::RgbImage {
    let cap: Option<u32> = match size {
        "original" => None,
        "large" => Some(2000),
        _ => Some(1200),
    };
    let Some(cap) = cap else { return img };
    if width as u32 <= cap && height as u32 <= cap {
        return img;
    }
    let aspect = width as f32 / height as f32;
    let (nw, nh) = if width > height {
        (cap, (cap as f32 / aspect) as u32)
    } else {
        ((cap as f32 * aspect) as u32, cap)
    };
    image::imageops::resize(&img, nw, nh, image::imageops::FilterType::Lanczos3)
}

/// Pixel dimension bucket for a preview `size` (shared by the preview handler
/// and the queue): `large` → 2000², `original` → none, else → 1200².
pub fn max_dimensions_for_size(size: &str) -> Option<(u32, u32)> {
    match size {
        "large" => Some((2000, 2000)),
        "original" => None,
        _ => Some((1200, 1200)),
    }
}

/// Bitmap-font scale for HFR labels drawn at native resolution, chosen so a
/// label still reads ~11 px tall after the image is downscaled to `size`
/// (glyphs are 7 px tall at scale 1). "original" is never downscaled, so a
/// small fixed scale keeps labels crisp without dwarfing the stars.
fn hfr_label_scale_for(native_width: u32, size: &str) -> u32 {
    match max_dimensions_for_size(size) {
        None => 2,
        Some((target_width, _)) => {
            let downscale = f64::from(native_width.max(1)) / f64::from(target_width);
            ((downscale * 11.0 / 7.0).round() as u32).clamp(2, 16)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn temp_path_is_unique_sibling() {
        let cache = PathBuf::from("/cache/previews/abc.png");
        let a = temp_path(&cache);
        let b = temp_path(&cache);
        assert_ne!(a, b, "temp paths must be unique per call");
        assert_eq!(a.parent(), cache.parent(), "temp file stays in cache dir");
        assert!(a.to_string_lossy().contains("abc.png.tmp."));
    }

    #[test]
    fn status_none_when_unknown() {
        let q = PreviewQueue::default();
        // Not cached, not in-flight, no error -> None (caller decides to enqueue).
        assert!(q.status(Path::new("/nonexistent/x.png")).is_none());
    }

    #[test]
    fn status_reports_in_flight_and_error() {
        let q = PreviewQueue::default();
        let p = PathBuf::from("/nonexistent/y.png");
        q.inner.lock().unwrap().in_flight.insert(p.clone());
        assert_eq!(q.status(&p).unwrap().state, GenerationState::Generating);

        q.inner.lock().unwrap().in_flight.remove(&p);
        record_error(
            &mut q.inner.lock().unwrap(),
            p.clone(),
            None,
            r"decoder failed at \\server\private\frame.fits".into(),
        );
        let s = q.status(&p).unwrap();
        assert_eq!(s.state, GenerationState::Error);
        assert_eq!(s.error.as_deref(), Some(GENERATION_ERROR_MESSAGE));
        assert!(!s.error.unwrap().contains("private"));
    }

    #[test]
    fn source_change_clears_a_cached_generation_error() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("arriving.fits");
        let cache = dir.path().join("preview.png");
        std::fs::write(&source, b"partial").unwrap();

        let q = PreviewQueue::default();
        q.inner.lock().unwrap().recent_errors.insert(
            cache.clone(),
            RecentError {
                source_fingerprint: source_fingerprint(&source),
            },
        );
        assert_eq!(
            q.status_for_source(&cache, &source).unwrap().state,
            GenerationState::Error
        );

        std::fs::write(&source, b"a larger complete frame").unwrap();
        assert!(q.status_for_source(&cache, &source).is_none());
        assert!(!q.inner.lock().unwrap().recent_errors.contains_key(&cache));
    }

    #[test]
    fn a_different_source_path_clears_an_equal_sized_failure() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("first.fits");
        let second = dir.path().join("second.fits");
        let cache = dir.path().join("preview.png");
        std::fs::write(&first, b"same bytes").unwrap();
        std::fs::write(&second, b"same bytes").unwrap();

        let q = PreviewQueue::default();
        q.inner.lock().unwrap().recent_errors.insert(
            cache.clone(),
            RecentError {
                source_fingerprint: source_fingerprint(&first),
            },
        );
        assert!(q.status_for_source(&cache, &second).is_none());
    }

    #[test]
    fn a_completed_artifact_is_invalid_after_its_source_grows() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("arriving.fits");
        let cache = dir.path().join("preview.png");
        std::fs::write(&source, b"stable prefix").unwrap();
        std::fs::write(&cache, b"rendered prefix").unwrap();

        let q = PreviewQueue::default();
        q.inner.lock().unwrap().completed_sources.insert(
            cache.clone(),
            source_fingerprint(&source).expect("source fingerprint"),
        );
        assert!(q.cached_source_matches(&cache, &source));

        std::fs::write(&source, b"stable prefix followed by the remaining pixels").unwrap();
        assert!(!q.cached_source_matches(&cache, &source));
    }

    #[test]
    fn recording_one_more_error_evicts_only_one_entry() {
        let mut inner = QueueInner::default();
        for index in 0..MAX_RECENT_ERRORS {
            inner.recent_errors.insert(
                PathBuf::from(format!("/cache/{index}.png")),
                RecentError {
                    source_fingerprint: None,
                },
            );
        }
        record_error(
            &mut inner,
            PathBuf::from("/cache/new.png"),
            None,
            "failed".into(),
        );
        assert_eq!(inner.recent_errors.len(), MAX_RECENT_ERRORS);
        assert!(inner
            .recent_errors
            .contains_key(Path::new("/cache/new.png")));
    }

    #[test]
    fn max_dimensions_buckets() {
        assert_eq!(max_dimensions_for_size("large"), Some((2000, 2000)));
        assert_eq!(max_dimensions_for_size("screen"), Some((1200, 1200)));
        assert_eq!(max_dimensions_for_size("original"), None);
        assert_eq!(max_dimensions_for_size("weird"), Some((1200, 1200)));
    }

    #[test]
    fn hfr_label_scale_tracks_the_downscale() {
        // 26MP-class frame at screen size: 6248/1200 ≈ 5.2× downscale, so
        // labels need scale 8 (56 px native → ~11 px on screen).
        assert_eq!(hfr_label_scale_for(6248, "screen"), 8);
        assert_eq!(hfr_label_scale_for(6248, "large"), 5);
        // No downscale: a small fixed scale.
        assert_eq!(hfr_label_scale_for(6248, "original"), 2);
        // A frame already at target size never drops below readable.
        assert_eq!(hfr_label_scale_for(1200, "screen"), 2);
        // 61MP-class frame stays within the clamp.
        assert_eq!(hfr_label_scale_for(9576, "screen"), 13);
    }
}
