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

#[derive(Default)]
struct QueueInner {
    /// `cache_path`s currently being generated (dedup).
    in_flight: HashSet<PathBuf>,
    /// `cache_path` -> last generation error message.
    recent_errors: HashMap<PathBuf, String>,
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
            .get(cache_path)
            .map(|e| GenerationStatus::error(e.clone()))
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
        // Dedup + claim the slot under the lock. `insert` returns false when
        // the path was already in-flight.
        {
            let mut inner = self.preview_queue.inner.lock().unwrap();
            if job.cache_path.exists() || !inner.in_flight.insert(job.cache_path.clone()) {
                return;
            }
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
            let outcome = tokio::task::spawn_blocking(move || generate(&job)).await;

            let mut inner = state.preview_queue.inner.lock().unwrap();
            inner.in_flight.remove(&cache_path);
            match outcome {
                Ok(Ok(())) => {
                    inner.recent_errors.remove(&cache_path);
                }
                Ok(Err(e)) => record_error(&mut inner, cache_path, e.to_string()),
                Err(join) => record_error(&mut inner, cache_path, format!("panicked: {join}")),
            }
        });
    }
}

fn record_error(inner: &mut QueueInner, cache_path: PathBuf, msg: String) {
    tracing::warn!(
        "🖼️ Preview generation failed for {}: {}",
        cache_path.display(),
        msg
    );
    if inner.recent_errors.len() >= MAX_RECENT_ERRORS {
        inner.recent_errors.clear();
    }
    inner.recent_errors.insert(cache_path, msg);
}

/// Generate one artifact to a unique temp path, then atomically rename into
/// place, so a concurrent `Path::exists` poll never observes a partial file.
fn generate(job: &GenJob) -> anyhow::Result<()> {
    let tmp = temp_path(&job.cache_path);
    let result = match &job.kind {
        GenKind::Preview {
            midtone,
            shadow,
            max_dimensions,
        } => crate::commands::stretch_to_png::stretch_to_png_with_resize(
            &job.fits_path.to_string_lossy(),
            Some(tmp.to_string_lossy().into_owned()),
            *midtone,
            *shadow,
            false, // logarithmic
            false, // invert
            *max_dimensions,
        ),
        GenKind::Annotated { max_stars, size } => {
            generate_annotated(&job.fits_path, &tmp, size, *max_stars)
        }
    };

    match result {
        Ok(()) => {
            std::fs::rename(&tmp, &job.cache_path)?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
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
) -> anyhow::Result<()> {
    use crate::commands::annotate_stars_common::create_annotated_image;
    use crate::image_analysis::FitsImage;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ColorType, ImageEncoder, Rgb};

    let fits = FitsImage::from_file(fits_path)?;
    let rgb = create_annotated_image(&fits, max_stars, 0.2, -2.8, Rgb([255, 255, 0]))?;
    let final_image = resize_rgb_for_size(rgb, fits.width, fits.height, size);

    let file = std::fs::File::create(out_path)?;
    let writer = std::io::BufWriter::new(file);
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);
    let (w, h) = final_image.dimensions();
    encoder.write_image(&final_image, w, h, ColorType::Rgb8.into())?;
    Ok(())
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
        q.inner
            .lock()
            .unwrap()
            .recent_errors
            .insert(p.clone(), "boom".into());
        let s = q.status(&p).unwrap();
        assert_eq!(s.state, GenerationState::Error);
        assert_eq!(s.error.as_deref(), Some("boom"));
    }

    #[test]
    fn max_dimensions_buckets() {
        assert_eq!(max_dimensions_for_size("large"), Some((2000, 2000)));
        assert_eq!(max_dimensions_for_size("screen"), Some((1200, 1200)));
        assert_eq!(max_dimensions_for_size("original"), None);
        assert_eq!(max_dimensions_for_size("weird"), Some((1200, 1200)));
    }
}
