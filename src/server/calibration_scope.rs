//! The Calibration page's project scope: the sessions, masters and flat
//! groups one project's lights use (see
//! [`crate::calibration::project_scope`]).
//!
//! Working it out matches each light's header against the library. The
//! headers come from those the calibration health pass keeps. A light the
//! pass has not reached is read by a background job sized like any other
//! interactive work ([`crate::concurrency`]); the scope says how many are
//! pending and grows as they arrive, and the read stops once no view has
//! asked for a minute. No request reads a header. One runner per catalog
//! ([`CatalogJobs`]) reads headers in chunks and refreshes scopes between
//! them.
//!
//! Matching runs on a read-only connection of its own, off the async
//! workers, never on the shared request connection, and the result is kept
//! in memory under its revision
//! ([`crate::calibration::project_scope::scope_revision`]), which follows
//! only what can change the scope: grading the library in the background
//! leaves it alone. When the revision moves on, the last scope is served,
//! marked `refreshing`, while one background refresh works out the next;
//! requests for one project share a refresh.

use crate::calibration::project_scope::{self, ProjectScope};
use crate::calibration::{KeptHeader, ProjectLight};
use crate::commands::import::headers::FrameMeta;
use crate::concurrency::Stop;
use crate::concurrency::{ComputePool, Priority};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Scopes kept at most per catalog; the oldest go first.
const KEPT_SCOPES: usize = 32;

/// A project's header read stops once no view has asked for its scope
/// this long.
const IDLE_STOP: Duration = Duration::from_secs(60);

/// Lights whose headers one turn of the runner reads, so a refresh of the
/// scope runs between turns and the scope grows as headers arrive.
const READ_CHUNK: usize = 32;

/// What one kept scope was worked out from.
type Revision = String;

/// Headers the background job read for one project's lights, by light:
/// where the file was, and what it said.
type ReadHeaders = BTreeMap<i32, (PathBuf, FrameMeta)>;

/// Scopes, header reads and refreshes, run on one thread per catalog.
pub(crate) struct ScopeWork;

type Job = CatalogJob<ScopeWork>;

/// One catalog's scopes and the work that keeps them current.
#[derive(Default)]
pub(crate) struct Queue {
    /// Kept scopes by project, with their revision and when each was last
    /// used.
    kept: HashMap<i32, (Revision, u64, Arc<ProjectScope>)>,
    clock: u64,
    /// Headers read for lights the health pass has not kept, by project.
    read: HashMap<i32, ReadHeaders>,
    /// Header reads under way, by project.
    reading: HashMap<i32, Reading>,
    /// Scopes to refresh, in the order asked.
    refreshes: VecDeque<(i32, Arc<DatabaseContext>)>,
    /// Projects whose refresh is queued or running.
    refreshing: HashSet<i32>,
    /// One lock per project, so the page's lists, asked at once, share one
    /// working out instead of each doing it.
    working: HashMap<i32, Arc<Mutex<()>>>,
}

/// One project's header read.
struct Reading {
    /// Lights not read yet.
    lights: VecDeque<ProjectLight>,
    /// How many lights the read set out to read.
    total: usize,
    /// When a view last asked for the project's scope.
    asked: Instant,
}

/// One turn of the runner.
pub(crate) enum Item {
    /// Work out a project's scope again.
    Refresh(i32, Arc<DatabaseContext>),
    /// Read the headers of some of a project's lights.
    Read {
        project: i32,
        lights: Vec<ProjectLight>,
    },
}

impl CatalogWork for ScopeWork {
    type Queue = Queue;
    type Status = ();
    type Item = Item;
    const RUNNER: &'static str = "calibration-scope";
    /// It reads headers and the catalog, and writes nothing that a stop
    /// must wait for.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = None;

    /// Refreshes go first, so a scope follows the headers read so far.
    /// Header reads no view has asked for in [`IDLE_STOP`] are dropped.
    fn next(_: &AppState, _: &str, queue: &mut Queue, now: Instant) -> Step<Item> {
        if let Some((project, ctx)) = queue.refreshes.pop_front() {
            return Step::Run(Item::Refresh(project, ctx));
        }
        queue
            .reading
            .retain(|_, reading| now.saturating_duration_since(reading.asked) <= IDLE_STOP);
        queue
            .reading
            .retain(|_, reading| !reading.lights.is_empty());
        let Some((&project, reading)) = queue.reading.iter_mut().next() else {
            return Step::Exit;
        };
        let take = reading.lights.len().min(READ_CHUNK);
        let lights = reading.lights.drain(..take).collect();
        Step::Run(Item::Read { project, lights })
    }

    fn run(state: &Arc<AppState>, path: &str, job: &Job, item: Item, _: &Stop) {
        match item {
            Item::Refresh(project, ctx) => {
                let outcome = read_only(&ctx.database_path)
                    .and_then(|conn| revision(&conn, job, project))
                    .and_then(|revision| work_out(state, job, &ctx, project, revision));
                if let Err(error) = outcome {
                    tracing::warn!("Could not work out project {project}'s calibration: {error:#}");
                }
                job.queue().refreshing.remove(&project);
            }
            Item::Read { project, lights } => read_headers(state, path, job, project, &lights),
        }
    }

    fn has_work(queue: &Queue) -> bool {
        !queue.refreshes.is_empty()
            || queue
                .reading
                .values()
                .any(|reading| !reading.lights.is_empty())
    }

    /// Forget the work in hand; the next view asks for it again.
    fn runner_failed(queue: &mut Queue, _: &mut ()) {
        queue.refreshes.clear();
        queue.refreshing.clear();
        queue.reading.clear();
    }
}

/// The job of the catalog at `path`.
fn job(state: &AppState, path: &str) -> Arc<Job> {
    state.catalog_jobs.calibration_scope.job(path)
}

/// How far the header reads of the catalog at `path` have got: lights read
/// and lights to read, across its projects. `None` when none runs.
pub(crate) fn header_reads(state: &AppState, path: &str) -> Option<(usize, usize)> {
    let job = state.catalog_jobs.calibration_scope.existing(path)?;
    let queue = job.queue();
    let (left, total) = queue
        .reading
        .values()
        .fold((0, 0), |(left, total), reading| {
            (left + reading.lights.len(), total + reading.total)
        });
    (total > 0 && (left > 0 || queue.running())).then(|| (total - left, total))
}

fn working_lock(job: &Job, project: i32) -> Arc<Mutex<()>> {
    let mut queue = job.queue();
    // Locks no one holds or waits for go, so the map stays small.
    queue.working.retain(|_, lock| Arc::strong_count(lock) > 1);
    Arc::clone(queue.working.entry(project).or_default())
}

fn read_only(path: &str) -> anyhow::Result<Connection> {
    Ok(
        crate::server::database_context::open_scheduler_connection_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?,
    )
}

fn read_count(job: &Job, project: i32) -> usize {
    job.queue().read.get(&project).map_or(0, BTreeMap::len)
}

/// The revision a project's scope depends on: the catalog's inputs, and
/// which headers are known.
fn revision(conn: &Connection, job: &Job, project: i32) -> anyhow::Result<Revision> {
    let catalog = project_scope::scope_revision(conn, project)?;
    let kept = crate::light_headers::project_stamp(conn, project);
    let read = read_count(job, project);
    Ok(format!("{catalog}:{kept}:{read}"))
}

/// A kept scope: whether it is of `revision`, and the scope.
fn kept(job: &Job, project: i32) -> Option<(Revision, Arc<ProjectScope>)> {
    let mut queue = job.queue();
    queue.clock += 1;
    let now = queue.clock;
    let entry = queue.kept.get_mut(&project)?;
    entry.1 = now;
    Some((entry.0.clone(), Arc::clone(&entry.2)))
}

fn keep(job: &Job, project: i32, revision: Revision, scope: Arc<ProjectScope>) {
    let mut queue = job.queue();
    queue.clock += 1;
    let now = queue.clock;
    queue.kept.insert(project, (revision, now, scope));
    while queue.kept.len() > KEPT_SCOPES {
        let Some(oldest) = queue
            .kept
            .iter()
            .min_by_key(|(_, (_, used, _))| *used)
            .map(|(project, _)| *project)
        else {
            break;
        };
        queue.kept.remove(&oldest);
        queue.read.remove(&oldest);
    }
}

/// The headers of a project's lights: the one the health pass kept, or
/// the job read, when its file is where it was found. Lights with neither
/// are returned to be read; unreadable ones are counted.
fn light_headers(
    lights: &[ProjectLight],
    kept: &BTreeMap<i32, KeptHeader>,
    read: &ReadHeaders,
) -> (Vec<FrameMeta>, usize, Vec<ProjectLight>) {
    let mut metas = Vec::with_capacity(lights.len());
    let mut unread = 0;
    let mut wanted = Vec::new();
    for light in lights {
        let meta = match (kept.get(&light.image_id), read.get(&light.image_id)) {
            (Some(header), _) if header.path == light.path => &header.meta,
            (_, Some((path, meta))) if *path == light.path => meta,
            _ => {
                wanted.push(light.clone());
                continue;
            }
        };
        if meta.readable {
            metas.push(meta.clone());
        } else {
            unread += 1;
        }
    }
    (metas, unread, wanted)
}

/// Mark a project's header read as wanted now. Whether one runs.
fn touch_reading(job: &Job, project: i32) -> bool {
    match job.queue().reading.get_mut(&project) {
        Some(reading) => {
            reading.asked = Instant::now();
            true
        }
        None => false,
    }
}

/// Read the headers of `lights` in the background, unless a read for this
/// project runs already, which is marked as wanted now instead.
fn start_reading(state: &Arc<AppState>, path: &str, project: i32, lights: Vec<ProjectLight>) {
    let scopes = &state.catalog_jobs.calibration_scope;
    scopes.request(state, path, |queue| match queue.reading.get_mut(&project) {
        Some(reading) => reading.asked = Instant::now(),
        None => {
            let total = lights.len();
            queue.reading.insert(
                project,
                Reading {
                    lights: lights.into(),
                    total,
                    asked: Instant::now(),
                },
            );
        }
    });
}

/// Read some of a project's lights' headers, sized as interactive work: a
/// view waits for them. It stops between lights once the server stops.
fn read_headers(state: &AppState, path: &str, job: &Job, project: i32, lights: &[ProjectLight]) {
    let budget =
        crate::concurrency::plan_workers(None, &state.worker_policy(), Priority::Interactive, None);
    let lease = state.lease_workers(Priority::Interactive, budget.workers);
    let pool = match ComputePool::take("calibration-scope-headers", lease.workers) {
        Ok(pool) => pool,
        Err(error) => {
            tracing::warn!(
                "Could not read the light headers of project {project} in {path}: {error}"
            );
            return;
        }
    };
    let read: Vec<(i32, PathBuf, FrameMeta)> = pool.install(|| {
        lights
            .par_iter()
            .filter(|_| !state.shutdown.is_stopping())
            .map(|light| {
                let meta = crate::commands::import::headers::read_frame_meta(&light.path);
                (light.image_id, light.path.clone(), meta)
            })
            .collect()
    });
    let mut queue = job.queue();
    let headers = queue.read.entry(project).or_default();
    for (image_id, path, meta) in read {
        headers.insert(image_id, (path, meta));
    }
}

/// Work out one project's scope at `revision`, and keep it. Blocking.
fn work_out(
    state: &Arc<AppState>,
    job: &Job,
    ctx: &DatabaseContext,
    project: i32,
    revision: Revision,
) -> anyhow::Result<Arc<ProjectScope>> {
    let lock = working_lock(job, project);
    let _working = lock.lock().unwrap_or_else(|p| p.into_inner());
    // Another request may have worked it out while this one waited.
    if let Some((kept_revision, scope)) = kept(job, project)
        && kept_revision == revision
    {
        return Ok(scope);
    }
    let conn = read_only(&ctx.database_path)?;
    let tree = ctx.get_directory_tree()?;
    // Lights' nights as Images names them, at the server's setting.
    let nights = crate::calibration::LightNights::load(&conn, None);
    let (lights, missing) = crate::calibration::project_lights(&conn, project, &tree, &nights)?;
    let headers = crate::light_headers::project_headers(&conn, project).unwrap_or_default();
    let read = job.queue().read.get(&project).cloned().unwrap_or_default();
    let (metas, unread, wanted) = light_headers(&lights, &headers, &read);
    let pending = wanted.len();
    if !wanted.is_empty() {
        start_reading(state, &ctx.database_path, project, wanted);
    }
    let mut scope = project_scope::project_scope(&conn, project, &metas, missing + unread)?;
    scope.pending = pending;
    scope.revision = revision.clone();
    let scope = Arc::new(scope);
    keep(job, project, revision, Arc::clone(&scope));
    Ok(scope)
}

/// Refresh a scope in the background, unless a refresh is queued or runs.
fn start_refresh(state: &Arc<AppState>, ctx: &Arc<DatabaseContext>, project: i32) {
    let scopes = &state.catalog_jobs.calibration_scope;
    scopes.request(state, &ctx.database_path, |queue| {
        if queue.refreshing.insert(project) {
            queue.refreshes.push_back((project, Arc::clone(ctx)));
        }
    });
}

/// One project's scope for a view: the kept one when current, the last
/// one while a refresh runs, or worked out now when there is none.
/// Blocking: call it off the async workers.
pub(crate) fn scope_of(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project: i32,
) -> anyhow::Result<Arc<ProjectScope>> {
    let job = job(state, &ctx.database_path);
    let conn = read_only(&ctx.database_path)?;
    let revision = revision(&conn, &job, project)?;
    drop(conn);
    match kept(&job, project) {
        Some((kept_revision, scope)) if kept_revision == revision => {
            // Still waiting on headers: keep their read going, or start it
            // again after it stopped for want of a view.
            if scope.pending > 0 && !touch_reading(&job, project) {
                start_refresh(state, ctx, project);
            }
            Ok(scope)
        }
        Some((_, scope)) => {
            start_refresh(state, ctx, project);
            touch_reading(&job, project);
            let mut stale = (*scope).clone();
            stale.refreshing = true;
            Ok(Arc::new(stale))
        }
        None => work_out(state, &job, ctx, project, revision),
    }
}

/// A project's scope for a request, worked out off the async workers.
pub(crate) async fn scope_for(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project: i32,
) -> Result<Arc<ProjectScope>, AppError> {
    let (state, ctx) = (Arc::clone(state), Arc::clone(ctx));
    tokio::task::spawn_blocking(move || scope_of(&state, &ctx, project))
        .await
        .map_err(|error| {
            AppError::InternalError(format!("calibration scope task failed: {error}"))
        })?
        .map_err(|error| {
            AppError::InternalError(format!("working out the project's calibration: {error:#}"))
        })
}

#[derive(Debug, Deserialize)]
pub struct ScopeQuery {
    pub project: i32,
}

/// GET /api/db/{db}/calibrations/project-scope?project= — what one
/// project's lights use from the library: its sessions, masters and rigs,
/// with how many lights' headers are still being read.
pub async fn get_project_scope(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<ScopeQuery>,
) -> Result<Json<ApiResponse<ProjectScope>>, AppError> {
    let scope = scope_for(&state, &ctx.0, query.project).await?;
    Ok(Json(ApiResponse::success((*scope).clone())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_scope_goes_first_with_the_headers_read_for_it() {
        let job = Job::default();
        for project in 0..KEPT_SCOPES as i32 {
            keep(&job, project, "r".into(), Arc::default());
        }
        job.queue()
            .read
            .entry(1)
            .or_default()
            .insert(1, (PathBuf::from("a.fits"), FrameMeta::default()));
        assert!(kept(&job, 0).is_some(), "used again, so kept longest");
        keep(&job, 999, "r".into(), Arc::default());
        assert!(kept(&job, 0).is_some());
        assert!(kept(&job, 1).is_none(), "the least recently used went");
        assert_eq!(read_count(&job, 1), 0, "and its headers with it");
    }

    fn light(image_id: i32, path: &str) -> ProjectLight {
        ProjectLight {
            image_id,
            image_guid: None,
            path: PathBuf::from(path),
            night: "2026-09-10".into(),
            filter: "Ha".into(),
        }
    }

    #[test]
    fn refreshes_go_before_header_reads_which_run_in_chunks_until_no_view_asks() {
        let state = AppState::new_for_test(Connection::open_in_memory().unwrap());
        let ctx = Arc::new(DatabaseContext::new_for_test(
            Connection::open_in_memory().unwrap(),
        ));
        let start = Instant::now();
        let mut queue = Queue::default();
        queue.reading.insert(
            7,
            Reading {
                lights: (0..READ_CHUNK as i32 + 5)
                    .map(|id| light(id, "a.fits"))
                    .collect(),
                total: READ_CHUNK + 5,
                asked: start,
            },
        );
        queue.refreshing.insert(7);
        queue.refreshes.push_back((7, ctx));
        let next = |queue: &mut Queue, now| ScopeWork::next(&state, "c", queue, now);
        assert!(matches!(
            next(&mut queue, start),
            Step::Run(Item::Refresh(7, _))
        ));
        match next(&mut queue, start) {
            Step::Run(Item::Read { project: 7, lights }) => assert_eq!(lights.len(), READ_CHUNK),
            _ => panic!("the first chunk was not read"),
        }
        // No view has asked for a minute: the rest is dropped.
        assert!(matches!(
            next(&mut queue, start + IDLE_STOP + Duration::from_secs(1)),
            Step::Exit
        ));
        assert!(!ScopeWork::has_work(&queue));
    }

    #[test]
    fn a_failed_runner_forgets_the_work_in_hand() {
        let ctx = Arc::new(DatabaseContext::new_for_test(
            Connection::open_in_memory().unwrap(),
        ));
        let mut queue = Queue::default();
        queue.refreshing.insert(3);
        queue.refreshes.push_back((3, ctx));
        ScopeWork::runner_failed(&mut queue, &mut ());
        assert!(queue.refreshing.is_empty() && queue.refreshes.is_empty());
    }

    #[test]
    fn a_light_is_read_only_when_no_header_is_known_where_its_file_is() {
        let readable = FrameMeta {
            readable: true,
            ..FrameMeta::default()
        };
        let kept = BTreeMap::from([(
            1,
            KeptHeader {
                path: PathBuf::from("one.fits"),
                size: 1,
                modified_ns: 1,
                changed_ns: 1,
                meta: readable.clone(),
                located: false,
                image_guid: None,
            },
        )]);
        let read = BTreeMap::from([
            (2, (PathBuf::from("two.fits"), readable.clone())),
            (3, (PathBuf::from("three.fits"), FrameMeta::default())),
            (4, (PathBuf::from("old/four.fits"), readable)),
        ]);
        let lights = [
            light(1, "one.fits"),
            light(2, "two.fits"),
            light(3, "three.fits"),
            light(4, "new/four.fits"),
            light(5, "five.fits"),
        ];
        let (metas, unread, wanted) = light_headers(&lights, &kept, &read);
        assert_eq!(metas.len(), 2, "kept by the pass, and read by the job");
        assert_eq!(
            unread, 1,
            "a header that could not be read is not read again"
        );
        let wanted: Vec<i32> = wanted.iter().map(|light| light.image_id).collect();
        assert_eq!(wanted, [4, 5], "a moved file, and one never read");
    }
}
