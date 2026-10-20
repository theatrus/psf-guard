//! Preview pre-generation: writing the previews a viewer will ask for before
//! anyone opens them, one runner thread per catalog.
//!
//! A server started with pre-generation on requests a pass for every
//! catalog at start and every five minutes ([`spawn`]). A pass lists the
//! catalog's images and keeps those whose enabled previews are missing or
//! older than the cache expiry. It renders them in batches on leased
//! background workers. Before each batch the pass waits while a person's
//! work runs, and the pass ends when the cache volume nears its limit.
//!
//! The header lists a pass that has previews to write. Stop ends the pass,
//! and no further pass runs for that catalog until the server restarts. A
//! preview someone opens is still made at once by the preview queue
//! (`preview_queue.rs`).

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use rayon::prelude::*;
use serde::Serialize;

use crate::concurrency::{ComputePool, Priority, Stop};
use crate::models::AcquiredImage;
use crate::preview_format::PreviewEncoding;
use crate::server::database_context::DatabaseContext;
use crate::server::handlers;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::preview_queue::{max_dimensions_for_size, GenJob, GenKind};
use crate::server::state::AppState;

/// The stretch pre-generation renders with. It matches the request path's
/// defaults, because an artifact rendered with anything else is one the
/// viewer never asks for.
pub(crate) const MIDTONE: f64 = 0.2;
pub(crate) const SHADOW: f64 = -2.8;
/// How often the timer requests a pass for each catalog.
const PASS_EVERY: Duration = Duration::from_secs(300);
/// How often a pass that waits for a person's work checks again.
const POLL: Duration = Duration::from_millis(500);
/// The annotated image pre-generation writes: the size and star count the
/// viewer asks for by default.
const ANNOTATED_SIZE: &str = "screen";
const ANNOTATED_MAX_STARS: usize = 1000;

/// A catalog's pending pass.
#[derive(Default)]
pub(crate) struct Queue {
    /// A pass waits to run.
    pending: bool,
    /// Stop ended a pass: no further pass runs until the server restarts.
    stopped: bool,
}

/// Where a catalog's pre-generation stands, for the header.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PregenerationStatus {
    /// Whether a pass runs.
    pub active: bool,
    /// Images this pass writes previews for. 0 while the pass checks which
    /// images need them.
    pub total: usize,
    /// Images done, written or failed.
    pub done: usize,
    /// Previews written.
    pub written: usize,
    /// Previews that could not be written.
    pub failed: usize,
    /// The person's work the pass waits for, if it waits.
    pub waiting_for: Option<String>,
}

/// Preview pre-generation, run on one thread per catalog. Catalogs are keyed
/// by database id.
pub(crate) struct Pregeneration;

type Job = CatalogJob<Pregeneration>;

impl CatalogWork for Pregeneration {
    type Queue = Queue;
    type Status = PregenerationStatus;
    type Item = ();
    const RUNNER: &'static str = "preview-pregeneration";
    /// The batch in hand finishes before the server stops, so no preview is
    /// left half written; no new batch starts.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("preview pre-generation");

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<()> {
        if std::mem::take(&mut queue.pending) {
            Step::Run(())
        } else {
            Step::Exit
        }
    }

    fn run(state: &Arc<AppState>, catalog: &str, job: &Job, (): (), stop: &Stop) {
        run_logged(state, catalog, job, stop);
    }

    fn has_work(queue: &Queue) -> bool {
        queue.pending
    }

    fn runner_failed(queue: &mut Queue, status: &mut PregenerationStatus) {
        queue.pending = false;
        status.active = false;
        status.waiting_for = None;
    }
}

/// Request pre-generation for every catalog at start and every five minutes,
/// when the server was started with it on.
pub fn spawn(state: &Arc<AppState>) {
    if !state.pregeneration_config.is_enabled() {
        return;
    }
    tracing::info!("🎨 Preview pre-generation is on");
    crate::server::schedule::every(
        state,
        "preview pre-generation",
        crate::server::schedule::Every::after(Duration::ZERO, PASS_EVERY),
        |state| async move {
            for ctx in state.all_databases() {
                request(&state, &ctx.id);
            }
        },
    );
}

/// Request a pass for the catalog with database id `catalog`. Nothing
/// happens while a pass runs, or after Stop.
pub fn request(state: &Arc<AppState>, catalog: &str) {
    if state.shutdown.is_stopping() {
        return;
    }
    let jobs = &state.catalog_jobs.pregeneration;
    if jobs.job(catalog).queue().running() {
        return;
    }
    jobs.request(state, catalog, |queue| {
        if !queue.stopped {
            queue.pending = true;
        }
    });
}

/// Stop the catalog's pass, and start no further pass for it until the
/// server restarts. Returns true when a pass was running.
pub fn stop(state: &AppState, catalog: &str) -> bool {
    state
        .catalog_jobs
        .pregeneration
        .existing(catalog)
        .is_some_and(|job| {
            job.stop(|queue| {
                queue.pending = false;
                queue.stopped = true;
            })
        })
}

/// Where the catalog's pre-generation stands.
pub fn status(state: &AppState, catalog: &str) -> PregenerationStatus {
    state
        .catalog_jobs
        .pregeneration
        .existing(catalog)
        .map(|job| job.status())
        .unwrap_or_default()
}

/// `812 of 4,000 images`.
pub fn label(done: usize, total: usize) -> String {
    use crate::server::job_text::{grouped, plural};
    format!(
        "{} of {} {}",
        grouped(done.min(total)),
        grouped(total),
        plural(total, "image", "images")
    )
}

/// `POST /api/db/{db}/preview-pregeneration/stop`: stop the database's pass.
/// Returns true when a pass was running.
pub async fn post_stop(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
    ctx: crate::server::extract::DbContext,
) -> axum::Json<crate::server::api::ApiResponse<bool>> {
    axum::Json(crate::server::api::ApiResponse::success(stop(
        &state, &ctx.id,
    )))
}

fn run_logged(state: &Arc<AppState>, catalog: &str, job: &Job, stop: &Stop) {
    let Some(ctx) = state.get_database(catalog) else {
        return;
    };
    job.update(|status| {
        *status = PregenerationStatus {
            active: true,
            ..Default::default()
        }
    });
    match run(state, &ctx, job, stop) {
        Ok(()) => {
            let status = job.status();
            if status.total > 0 {
                tracing::info!(
                    "🎨 Pre-generated previews for {} of {} images (db={}): {} written, {} failed{}",
                    status.done,
                    status.total,
                    ctx.id,
                    status.written,
                    status.failed,
                    if stop.requested() { "; stopped" } else { "" }
                );
            }
        }
        Err(error) => tracing::error!(
            "🎨 Preview pre-generation failed (db={}): {error:#}",
            ctx.id
        ),
    }
    job.update(|status| {
        status.active = false;
        status.waiting_for = None;
    });
}

fn run(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    job: &Job,
    stop: &Stop,
) -> anyhow::Result<()> {
    // Previews written now would be culled by the next pass of the cache
    // budget and written again by the next pre-generation pass.
    if !room_for_previews(state, ctx) {
        tracing::debug!(
            "🎨 Pre-generation skips this pass (db={}): the cache volume is near its limit",
            ctx.id
        );
        return Ok(());
    }
    let pending = pending_images(state, ctx)?;
    if pending.is_empty() {
        return Ok(());
    }
    job.update(|status| status.total = pending.len());
    // Pre-generation loads full frames, so the memory ceiling of a frame
    // this size applies.
    let frame_pixels = probe_frame_pixels(ctx, &pending);
    let budget = crate::concurrency::plan_workers(
        None,
        &state.worker_policy(),
        Priority::Background,
        frame_pixels,
    );
    tracing::info!(
        "🎨 Pre-generating previews for {} images (db={}) — {}",
        pending.len(),
        ctx.id,
        budget.rationale
    );
    let encoding = state.preview_encoding();
    for batch in pending.chunks(budget.workers.max(1) * 2) {
        if !may_continue(state, ctx, job, stop) {
            break;
        }
        // Leased per batch, so a pass that waits for a person's work holds
        // no workers.
        let lease = state.lease_workers(Priority::Background, budget.workers.min(batch.len()));
        let pool = ComputePool::take("preview-pregeneration", lease.workers)?;
        let rows = image_rows(ctx, batch)?;
        let rendered: Vec<Option<Rendered>> = pool.install(|| {
            batch
                .par_iter()
                .map(|image| {
                    // Stop and a server stop reach the images of the batch
                    // that have not started.
                    if stop.requested() || state.shutdown.is_stopping() {
                        return None;
                    }
                    // An image deleted since the pass listed it needs no
                    // preview.
                    Some(match rows.get(&image.image_id) {
                        Some(row) => render(ctx, image, row, encoding),
                        None => Rendered::default(),
                    })
                })
                .collect()
        });
        job.update(|status| {
            for image in rendered.iter().flatten() {
                status.done += 1;
                status.written += image.written;
                status.failed += image.failed;
            }
        });
    }
    Ok(())
}

/// Whether the pass starts its next batch. It waits while a person's work
/// runs, and ends on Stop, at server shutdown, or when the cache volume
/// nears its limit.
fn may_continue(state: &AppState, ctx: &DatabaseContext, job: &Job, stop: &Stop) -> bool {
    let running = state.wait_for_interactive_work_unless(
        POLL,
        || stop.requested(),
        |waiting_for| job.update(|status| status.waiting_for = waiting_for.map(str::to_string)),
    );
    running && !stop.requested() && room_for_previews(state, ctx)
}

fn room_for_previews(state: &AppState, ctx: &DatabaseContext) -> bool {
    crate::server::cache_budget::room_for_previews(&state.cache_budget, &ctx.cache_dir_path)
}

/// An image with previews to write. Its database row is read again for its
/// batch, so a long pass does not keep every row and its metadata.
struct Pending {
    image_id: i32,
    file_only: String,
    target_name: String,
    /// Each preview to write: where it goes and how to render it.
    previews: Vec<(PathBuf, GenKind)>,
}

/// The catalog's images whose enabled previews are missing or older than
/// the cache expiry. The cache paths are the ones the request path builds,
/// so the viewer finds what this writes.
fn pending_images(state: &AppState, ctx: &DatabaseContext) -> anyhow::Result<Vec<Pending>> {
    let config = &state.pregeneration_config;
    let encoding = state.preview_encoding();
    // Warm whichever rendition this server serves by default.
    let color = state.preview_color_default();
    let sizes = [
        ("screen", config.screen_enabled),
        ("large", config.large_enabled),
        ("original", config.original_enabled),
    ];
    // `with_db` reopens the connection when the scheduler database was
    // replaced under it, so a pass after a replacement still runs.
    let images = ctx.with_db(|db| {
        db.query_images(None, None, None, None)
            .context("listing images for preview pre-generation")
    })?;
    let mut pending = Vec::new();
    for (image, _project, target_name) in images {
        let Some(file_only) = file_name(&image) else {
            continue;
        };
        let revision = match handlers::rendered_artifact_mapping_revision(ctx, &image) {
            Ok(revision) => revision,
            Err(error) => {
                tracing::debug!(
                    "🎨 Image {} (db={}) is left out of pre-generation: {error:?}",
                    image.id,
                    ctx.id
                );
                continue;
            }
        };
        let path = |category: &str, key: &str| {
            handlers::artifact_cache_path(ctx, category, key, encoding)
                .map_err(|error| anyhow::anyhow!("{error:?}"))
        };
        let mut previews = Vec::new();
        for (size, _) in sizes.iter().filter(|(_, enabled)| *enabled) {
            let key = handlers::preview_cache_key(
                &image,
                &file_only,
                size,
                true,
                MIDTONE,
                SHADOW,
                color,
                revision.as_deref(),
            );
            let cache_path = path("previews", &key)?;
            if needs_writing(&cache_path, config.cache_expiry) {
                previews.push((
                    cache_path,
                    GenKind::Preview {
                        midtone: MIDTONE,
                        shadow: SHADOW,
                        max_dimensions: max_dimensions_for_size(size),
                        color,
                    },
                ));
            }
        }
        if config.annotated_enabled {
            let key = handlers::annotated_cache_key(
                &image,
                &file_only,
                ANNOTATED_SIZE,
                ANNOTATED_MAX_STARS,
                revision.as_deref(),
            );
            let cache_path = path("annotated", &key)?;
            if needs_writing(&cache_path, config.cache_expiry) {
                previews.push((
                    cache_path,
                    GenKind::Annotated {
                        max_stars: ANNOTATED_MAX_STARS,
                        size: ANNOTATED_SIZE.to_string(),
                    },
                ));
            }
        }
        if !previews.is_empty() {
            pending.push(Pending {
                image_id: image.id,
                file_only,
                target_name,
                previews,
            });
        }
    }
    Ok(pending)
}

/// The frame's file name, from the `FileName` in its metadata.
fn file_name(image: &AcquiredImage) -> Option<String> {
    let metadata: serde_json::Value = serde_json::from_str(&image.metadata).ok()?;
    let path = metadata["FileName"].as_str()?;
    Some(path.rsplit(['\\', '/']).next().unwrap_or(path).to_string())
}

/// Whether the preview at `path` is missing or older than `expiry`.
fn needs_writing(path: &std::path::Path, expiry: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map_or(true, |modified| {
            modified.elapsed().unwrap_or_default() >= expiry
        })
}

/// The pixel count of the first pending frame on disk, found through the
/// directory tree without reading pixels. `None` when no frame resolves;
/// the budget then counts cores only.
fn probe_frame_pixels(ctx: &DatabaseContext, pending: &[Pending]) -> Option<usize> {
    let tree = ctx.get_directory_tree().ok()?;
    pending.iter().find_map(|image| {
        tree.find_file_first(&image.file_only)
            .and_then(|path| crate::concurrency::probe_frame_pixels(path))
    })
}

/// What rendering one image's previews came to.
#[derive(Default)]
struct Rendered {
    written: usize,
    failed: usize,
}

/// The database rows of a batch's images, by id.
fn image_rows(
    ctx: &DatabaseContext,
    batch: &[Pending],
) -> anyhow::Result<std::collections::HashMap<i32, AcquiredImage>> {
    let ids: Vec<i32> = batch.iter().map(|image| image.image_id).collect();
    let rows = ctx.with_db(|db| {
        db.get_images_by_ids(&ids)
            .context("reading images for preview pre-generation")
    })?;
    Ok(rows.into_iter().map(|row| (row.id, row)).collect())
}

fn render(
    ctx: &DatabaseContext,
    image: &Pending,
    row: &AcquiredImage,
    encoding: PreviewEncoding,
) -> Rendered {
    let fits_path = match handlers::find_fits_file(ctx, row, &image.target_name, &image.file_only) {
        Ok(path) => path,
        Err(error) => {
            tracing::warn!(
                "⚠️ Pre-generation found no file for image {} (db={}): {error:?}",
                image.image_id,
                ctx.id
            );
            return Rendered {
                written: 0,
                failed: image.previews.len(),
            };
        }
    };
    let mut rendered = Rendered::default();
    for (cache_path, kind) in &image.previews {
        // Written to a temporary file and renamed, so a viewer's poll never
        // reads a half-written preview.
        let job = GenJob {
            fits_path: fits_path.clone(),
            cache_path: cache_path.clone(),
            kind: kind.clone(),
            encoding,
        };
        match crate::server::preview_queue::generate(&job) {
            Ok(()) => rendered.written += 1,
            Err(error) => {
                rendered.failed += 1;
                tracing::warn!(
                    "⚠️ Pre-generation could not write {} for image {} (db={}): {error:#}",
                    cache_path.display(),
                    image.image_id,
                    ctx.id
                );
            }
        }
    }
    rendered
}

#[cfg(test)]
mod tests;
