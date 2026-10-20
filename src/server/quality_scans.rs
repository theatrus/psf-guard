//! Quality scans and the database-wide quality analysis, run on one thread
//! per catalog (see [`crate::server::job_line::catalog`]).
//!
//! A person's scan of one target and the analysis share the database's
//! scanner, so both wait in one queue per catalog. Scans a person requested
//! run first, in the order requested, at `Interactive` priority. The
//! analysis then scans its targets one at a time at `Background` priority,
//! and steps aside between frames for a person's work elsewhere. When a
//! person requests a scan while an analysis target is being scanned, that
//! scan stops at its next frame, keeps what it measured, and its target
//! goes back to the front of the analysis.
//!
//! The runner gathers the frames that need computing (see
//! [`crate::server::handlers::scan_target`]), so a request only checks that
//! its target exists, and returns at once.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;

use crate::concurrency::{Priority, Stop};
use crate::server::api::SpatialScanRequest;
use crate::server::database_context::DatabaseContext;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::state::AppState;
use crate::server::{quality_backfill, spatial_scan};

/// Quality scans as a job with one runner per catalog.
pub(crate) struct QualityScans;

type Job = CatalogJob<QualityScans>;

/// A scan a person requested.
pub(crate) struct PersonScan {
    ctx: Arc<DatabaseContext>,
    request: SpatialScanRequest,
    /// Satellite elements are fetched for a scan a person requested.
    runtime: tokio::runtime::Handle,
}

/// The database-wide analysis: the targets it has left to scan.
pub(crate) struct Analysis {
    ctx: Arc<DatabaseContext>,
    targets: VecDeque<i32>,
    force: bool,
    fill_metadata: bool,
    runtime: tokio::runtime::Handle,
}

/// What waits to be scanned in one catalog.
#[derive(Default)]
pub(crate) struct Queue {
    /// Scans people requested, oldest first.
    scans: VecDeque<PersonScan>,
    /// The analysis, while it has targets left or has not finished.
    analysis: Option<Analysis>,
    /// Whether the item running is one of the analysis's target scans.
    analysis_running: bool,
    /// The database of the item running, to finish its progress when the
    /// runner thread fails.
    running_ctx: Option<Arc<DatabaseContext>>,
}

/// One item for the runner thread.
pub(crate) enum Item {
    Scan(PersonScan),
    Target {
        ctx: Arc<DatabaseContext>,
        target_id: i32,
        force: bool,
        fill_metadata: bool,
        runtime: tokio::runtime::Handle,
    },
    /// The analysis scanned its last target.
    AnalysisDone(Arc<DatabaseContext>),
}

impl CatalogWork for QualityScans {
    type Queue = Queue;
    type Status = ();
    type Item = Item;
    const RUNNER: &'static str = "quality-scan";
    /// Each scan registers its own shutdown hold, labelled with its target
    /// and database: shutdown waits for the frames in hand and the save of
    /// what was measured.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<Item> {
        if let Some(scan) = queue.scans.pop_front() {
            queue.analysis_running = false;
            queue.running_ctx = Some(Arc::clone(&scan.ctx));
            // The previous scan may have just finished: the progress shows
            // this one from now on, while its frames are gathered.
            spatial_scan::begin_preparing(
                &scan.ctx.spatial_metrics,
                scan.request.target_id,
                scan.request.filter_name.clone(),
                false,
            );
            return Step::Run(Item::Scan(scan));
        }
        let Some(analysis) = &mut queue.analysis else {
            queue.analysis_running = false;
            queue.running_ctx = None;
            return Step::Exit;
        };
        queue.running_ctx = Some(Arc::clone(&analysis.ctx));
        match analysis.targets.pop_front() {
            Some(target_id) => {
                queue.analysis_running = true;
                Step::Run(Item::Target {
                    ctx: Arc::clone(&analysis.ctx),
                    target_id,
                    force: analysis.force,
                    fill_metadata: analysis.fill_metadata,
                    runtime: analysis.runtime.clone(),
                })
            }
            None => {
                queue.analysis_running = false;
                let done = queue.analysis.take().expect("the analysis is queued");
                Step::Run(Item::AnalysisDone(done.ctx))
            }
        }
    }

    fn run(state: &Arc<AppState>, _: &str, job: &Job, item: Item, stop: &Stop) {
        match item {
            Item::Scan(scan) => run_person_scan(state, scan, stop),
            Item::Target {
                ctx,
                target_id,
                force,
                fill_metadata,
                runtime,
            } => run_analysis_target(
                state,
                job,
                &ctx,
                target_id,
                force,
                fill_metadata,
                &runtime,
                stop,
            ),
            Item::AnalysisDone(ctx) => {
                quality_backfill::finish(&ctx.quality_backfill);
                // Scores changed under the remembered stacks; refresh them.
                state.auto_stacks.touch_database(
                    &ctx.id,
                    crate::server::stack_preview::automatic::RefreshReason::Arrival,
                );
                tracing::info!("📐 Database quality work finished for db={}", ctx.id);
            }
        }
        job.queue().running_ctx = None;
    }

    fn has_work(queue: &Queue) -> bool {
        !queue.scans.is_empty() || queue.analysis.is_some()
    }

    /// Finish the progress the failed thread left running: the item it ran,
    /// the queued scans shown as preparing, and the analysis, whose targets
    /// left are dropped.
    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        if let Some(ctx) = queue.running_ctx.take() {
            spatial_scan::end_preparing(&ctx.spatial_metrics);
        }
        for scan in queue.scans.drain(..) {
            spatial_scan::end_preparing(&scan.ctx.spatial_metrics);
        }
        if let Some(analysis) = queue.analysis.take() {
            quality_backfill::finish(&analysis.ctx.quality_backfill);
        }
        queue.analysis_running = false;
    }
}

/// The key of the job that scans for `ctx`: its scan progress store. A
/// database's contexts share one store, and the store shows one scan at a
/// time.
fn scanner(ctx: &DatabaseContext) -> String {
    format!("{:p}", Arc::as_ptr(&ctx.spatial_metrics))
}

/// Queue a person's scan of a target, and stop an analysis target scan that
/// is running, so the person's scan runs next. The same target and filter
/// requested again while queued is one scan. False while the server shuts
/// down. Call from the async runtime.
pub(crate) fn queue_scan(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    request: SpatialScanRequest,
) -> bool {
    if state.shutdown.is_stopping() {
        return false;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("No runtime to queue a quality scan on");
        return false;
    };
    spatial_scan::ensure_loaded(&ctx.spatial_metrics, &ctx.cache_dir_path);
    // A page polling the progress follows the scan from now on.
    spatial_scan::mark_preparing(
        &ctx.spatial_metrics,
        request.target_id,
        request.filter_name.clone(),
        false,
    );
    state
        .catalog_jobs
        .quality_scans
        .request(state, &scanner(ctx), |queue| {
            let same = queue.scans.iter_mut().find(|queued| {
                queued.request.target_id == request.target_id
                    && queued.request.filter_name == request.filter_name
            });
            match same {
                Some(queued) => merge(&mut queued.request, &request),
                None => queue.scans.push_back(PersonScan {
                    ctx: Arc::clone(ctx),
                    request,
                    runtime,
                }),
            }
        });
    state
        .catalog_jobs
        .quality_scans
        .job(&scanner(ctx))
        .stop_if(|queue| queue.analysis_running);
    true
}

/// Whether a person's scan waits in the queue of the store `ctx` scans
/// with: the progress then reads as running, though the scan before it has
/// finished.
pub(crate) fn scan_queued(state: &AppState, ctx: &DatabaseContext) -> bool {
    state
        .catalog_jobs
        .quality_scans
        .existing(&scanner(ctx))
        .is_some_and(|job| !job.queue().scans.is_empty())
}

/// A second request for a queued scan adds what it forces.
fn merge(queued: &mut SpatialScanRequest, request: &SpatialScanRequest) {
    queued.force |= request.force;
    queued.force_spatial |= request.force_spatial;
    queued.force_astrometry |= request.force_astrometry;
    queued.force_satellites |= request.force_satellites;
    if request.fill_metadata.is_some() {
        queued.fill_metadata = request.fill_metadata;
    }
}

/// Queue the database-wide analysis of `target_ids`. False when an analysis
/// is already queued or running, or while the server shuts down. Call from
/// the async runtime.
pub(crate) fn queue_analysis(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    target_ids: Vec<i32>,
    force: bool,
    fill_metadata: bool,
) -> bool {
    if state.shutdown.is_stopping() {
        return false;
    }
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("No runtime to queue a quality analysis on");
        return false;
    };
    let targets = target_ids.len();
    let queued = state
        .catalog_jobs
        .quality_scans
        .request(state, &scanner(ctx), |queue| {
            if queue.analysis.is_some()
                || !quality_backfill::try_begin(&ctx.quality_backfill, force, targets)
            {
                return false;
            }
            if targets > 0 {
                queue.analysis = Some(Analysis {
                    ctx: Arc::clone(ctx),
                    targets: target_ids.into(),
                    force,
                    fill_metadata,
                    runtime,
                });
            }
            true
        });
    if queued && targets > 0 {
        tracing::info!(
            "📐 Database quality {} queued for db={} ({targets} targets)",
            if force { "rescan" } else { "backfill" },
            ctx.id,
        );
    }
    queued
}

/// What shutdown waits for while a scan of `target_id` runs.
fn hold_shutdown(
    state: &AppState,
    ctx: &DatabaseContext,
    target_id: i32,
) -> crate::server::shutdown::WorkGuard {
    state.shutdown.hold(format!(
        "quality scan for target {target_id} (db={})",
        ctx.id
    ))
}

fn run_person_scan(state: &Arc<AppState>, scan: PersonScan, stop: &Stop) {
    let _shutdown_hold = hold_shutdown(state, &scan.ctx, scan.request.target_id);
    let outcome = crate::server::handlers::scan_target(
        state,
        &scan.ctx,
        &scan.request,
        Priority::Interactive,
        true,
        stop,
        &scan.runtime,
    );
    if let Err(error) = outcome {
        tracing::warn!(
            "📐 Quality scan for db={} target={} not run: {error:?}",
            scan.ctx.id,
            scan.request.target_id
        );
        spatial_scan::end_preparing(&scan.ctx.spatial_metrics);
    }
}

#[allow(clippy::too_many_arguments)]
fn run_analysis_target(
    state: &Arc<AppState>,
    job: &Job,
    ctx: &Arc<DatabaseContext>,
    target_id: i32,
    force: bool,
    fill_metadata: bool,
    runtime: &tokio::runtime::Handle,
    stop: &Stop,
) {
    let _shutdown_hold = hold_shutdown(state, ctx, target_id);
    quality_backfill::begin_target(&ctx.quality_backfill, target_id);
    let request = SpatialScanRequest {
        target_id,
        filter_name: None,
        force: false,
        force_spatial: force,
        force_astrometry: force,
        force_satellites: false,
        fill_metadata: Some(fill_metadata),
    };
    let outcome = crate::server::handlers::scan_target(
        state,
        ctx,
        &request,
        Priority::Background,
        false,
        stop,
        runtime,
    );
    if let Err(error) = outcome {
        tracing::warn!(
            "📐 Analysis scan for db={} target={target_id} not run: {error:?}",
            ctx.id
        );
    }
    // Stopped for a person's scan: the target is scanned again after it,
    // and what this scan measured is reused then. At shutdown the next
    // start's analysis scans the targets left.
    if stop.requested() && !state.shutdown.is_stopping() {
        if let Some(analysis) = job.queue().analysis.as_mut() {
            analysis.targets.push_front(target_id);
        }
        return;
    }
    quality_backfill::finish_target(&ctx.quality_backfill);
}

#[cfg(test)]
mod tests;
