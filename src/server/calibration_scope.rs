//! The Calibration page's project scope: the sessions, masters and flat
//! groups one project's lights use (see
//! [`crate::calibration::project_scope`]).
//!
//! Working it out matches each light's header against the library. The
//! headers come from those the calibration health pass keeps. A light the
//! pass has not reached is read by a background job sized like any other
//! interactive work ([`crate::concurrency`]); the scope says how many are
//! pending and grows as they arrive, and the job stops once no view has
//! asked for a minute. No request reads a header.
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
use crate::concurrency::{ComputePool, Priority};
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;
use axum::extract::{Query, State};
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Scopes kept at most, across every catalog; the oldest go first.
const KEPT_SCOPES: usize = 32;

/// A header job stops once no view has asked for its scope this long.
const IDLE_STOP: Duration = Duration::from_secs(60);

/// A catalog's path and a project.
type ScopeKey = (String, i32);

/// What one kept scope was worked out from.
type Revision = String;

#[derive(Default)]
struct Kept {
    /// With its revision and when it was last used.
    scopes: HashMap<ScopeKey, (Revision, u64, Arc<ProjectScope>)>,
    clock: u64,
}

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);

/// Headers the background job read for one project's lights, by light:
/// where the file was, and what it said.
type ReadHeaders = BTreeMap<i32, (PathBuf, FrameMeta)>;

/// Headers the background job read for lights the health pass has not
/// kept.
static READ: LazyLock<Mutex<HashMap<ScopeKey, ReadHeaders>>> = LazyLock::new(Mutex::default);

/// Header jobs running, with when a view last asked for their scope.
static READING: LazyLock<Mutex<HashMap<ScopeKey, Instant>>> = LazyLock::new(Mutex::default);

/// Scopes being refreshed in the background.
static REFRESHING: LazyLock<Mutex<HashSet<ScopeKey>>> = LazyLock::new(Mutex::default);

/// One lock per catalog and project, so the page's lists, asked at once,
/// share one working out instead of each doing it.
static WORKING: LazyLock<Mutex<HashMap<ScopeKey, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);

fn key(path: &str, project: i32) -> ScopeKey {
    (path.to_string(), project)
}

fn working_lock(key: &ScopeKey) -> Arc<Mutex<()>> {
    let mut locks = WORKING.lock().unwrap_or_else(|p| p.into_inner());
    // Locks no one holds or waits for go, so the map stays small.
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    Arc::clone(locks.entry(key.clone()).or_default())
}

fn read_only(path: &str) -> anyhow::Result<Connection> {
    Ok(
        crate::server::database_context::open_scheduler_connection_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?,
    )
}

fn read_count(key: &ScopeKey) -> usize {
    READ.lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(key)
        .map_or(0, BTreeMap::len)
}

/// The revision a project's scope depends on: the catalog's inputs, and
/// which headers are known.
fn revision(conn: &Connection, ctx: &DatabaseContext, project: i32) -> anyhow::Result<Revision> {
    let catalog = project_scope::scope_revision(conn, project)?;
    let kept = crate::light_headers::project_stamp(conn, project);
    let read = read_count(&key(&ctx.database_path, project));
    Ok(format!("{catalog}:{kept}:{read}"))
}

/// A kept scope: whether it is of `revision`, and the scope.
fn kept(key: &ScopeKey) -> Option<(Revision, Arc<ProjectScope>)> {
    let mut kept = KEPT.lock().unwrap_or_else(|p| p.into_inner());
    kept.clock += 1;
    let now = kept.clock;
    let entry = kept.scopes.get_mut(key)?;
    entry.1 = now;
    Some((entry.0.clone(), Arc::clone(&entry.2)))
}

fn keep(key: &ScopeKey, revision: Revision, scope: Arc<ProjectScope>) {
    let mut kept = KEPT.lock().unwrap_or_else(|p| p.into_inner());
    kept.clock += 1;
    let now = kept.clock;
    kept.scopes.insert(key.clone(), (revision, now, scope));
    while kept.scopes.len() > KEPT_SCOPES {
        let Some(oldest) = kept
            .scopes
            .iter()
            .min_by_key(|(_, (_, used, _))| *used)
            .map(|(key, _)| key.clone())
        else {
            break;
        };
        kept.scopes.remove(&oldest);
        READ.lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&oldest);
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

/// Mark a header job's scope as wanted now. Whether a job is running.
fn touch_reading(key: &ScopeKey) -> bool {
    let mut reading = READING.lock().unwrap_or_else(|p| p.into_inner());
    match reading.get_mut(key) {
        Some(asked) => {
            *asked = Instant::now();
            true
        }
        None => false,
    }
}

/// Read the headers of `lights` in the background, sized as interactive
/// work, unless a job for this scope runs already. It stops between lights
/// once the server stops or no view has asked for a minute.
fn start_reading(state: &Arc<AppState>, key: &ScopeKey, lights: Vec<ProjectLight>) {
    {
        let mut reading = READING.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(asked) = reading.get_mut(key) {
            *asked = Instant::now();
            return;
        }
        reading.insert(key.clone(), Instant::now());
    }
    let state = Arc::clone(state);
    let key = key.clone();
    let spawned = std::thread::Builder::new()
        .name("calibration-scope-headers".into())
        .spawn({
            let key = key.clone();
            move || {
                read_headers(&state, &key, &lights);
                READING
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&key);
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("Could not start reading a project's light headers: {error}");
        READING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
    }
}

fn read_headers(state: &AppState, key: &ScopeKey, lights: &[ProjectLight]) {
    let idle = || {
        READING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(key)
            .is_none_or(|asked| asked.elapsed() > IDLE_STOP)
    };
    let budget =
        crate::concurrency::plan_workers(None, &state.worker_policy(), Priority::Interactive, None);
    let lease = state.lease_workers(Priority::Interactive, budget.workers);
    let pool = match ComputePool::take("calibration-scope-headers", lease.workers) {
        Ok(pool) => pool,
        Err(error) => {
            tracing::warn!("Could not read a project's light headers: {error}");
            return;
        }
    };
    pool.install(|| {
        lights.par_iter().for_each(|light| {
            if state.shutdown.is_stopping() || idle() {
                return;
            }
            let meta = crate::commands::import::headers::read_frame_meta(&light.path);
            READ.lock()
                .unwrap_or_else(|p| p.into_inner())
                .entry(key.clone())
                .or_default()
                .insert(light.image_id, (light.path.clone(), meta));
        })
    });
}

/// Work out one project's scope at `revision`, and keep it. Blocking.
fn work_out(
    state: &Arc<AppState>,
    ctx: &DatabaseContext,
    project: i32,
    revision: Revision,
) -> anyhow::Result<Arc<ProjectScope>> {
    let key = key(&ctx.database_path, project);
    let lock = working_lock(&key);
    let _working = lock.lock().unwrap_or_else(|p| p.into_inner());
    // Another request may have worked it out while this one waited.
    if let Some((kept_revision, scope)) = kept(&key)
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
    let read = READ
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&key)
        .cloned()
        .unwrap_or_default();
    let (metas, unread, wanted) = light_headers(&lights, &headers, &read);
    let pending = wanted.len();
    if !wanted.is_empty() {
        start_reading(state, &key, wanted);
    }
    let mut scope = project_scope::project_scope(&conn, project, &metas, missing + unread)?;
    scope.pending = pending;
    scope.revision = revision.clone();
    let scope = Arc::new(scope);
    keep(&key, revision, Arc::clone(&scope));
    Ok(scope)
}

/// Refresh a scope in the background, unless a refresh runs already.
fn start_refresh(state: &Arc<AppState>, ctx: &Arc<DatabaseContext>, project: i32) {
    let key = key(&ctx.database_path, project);
    if !REFRESHING
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(key.clone())
    {
        return;
    }
    let (state, ctx) = (Arc::clone(state), Arc::clone(ctx));
    let spawned = std::thread::Builder::new()
        .name("calibration-scope".into())
        .spawn({
            let key = key.clone();
            move || {
                let outcome = read_only(&ctx.database_path)
                    .and_then(|conn| revision(&conn, &ctx, project))
                    .and_then(|revision| work_out(&state, &ctx, project, revision));
                if let Err(error) = outcome {
                    tracing::warn!("Could not work out project {project}'s calibration: {error:#}");
                }
                REFRESHING
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .remove(&key);
            }
        });
    if spawned.is_err() {
        REFRESHING
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key);
    }
}

/// One project's scope for a view: the kept one when current, the last
/// one while a refresh runs, or worked out now when there is none.
/// Blocking: call it off the async workers.
pub(crate) fn scope_of(
    state: &Arc<AppState>,
    ctx: &Arc<DatabaseContext>,
    project: i32,
) -> anyhow::Result<Arc<ProjectScope>> {
    let key = key(&ctx.database_path, project);
    let conn = read_only(&ctx.database_path)?;
    let revision = revision(&conn, ctx, project)?;
    drop(conn);
    match kept(&key) {
        Some((kept_revision, scope)) if kept_revision == revision => {
            // Still waiting on headers: keep their job going, or start it
            // again after it stopped for want of a view.
            if scope.pending > 0 && !touch_reading(&key) {
                start_refresh(state, ctx, project);
            }
            Ok(scope)
        }
        Some((_, scope)) => {
            start_refresh(state, ctx, project);
            touch_reading(&key);
            let mut stale = (*scope).clone();
            stale.refreshing = true;
            Ok(Arc::new(stale))
        }
        None => work_out(state, ctx, project, revision),
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
        let path = "kept-scope-test.sqlite";
        for project in 0..KEPT_SCOPES as i32 {
            keep(&key(path, project), "r".into(), Arc::default());
        }
        READ.lock()
            .unwrap()
            .entry(key(path, 1))
            .or_default()
            .insert(1, (PathBuf::from("a.fits"), FrameMeta::default()));
        assert!(kept(&key(path, 0)).is_some(), "used again, so kept longest");
        keep(&key(path, 999), "r".into(), Arc::default());
        assert!(kept(&key(path, 0)).is_some());
        assert!(
            kept(&key(path, 1)).is_none(),
            "the least recently used went"
        );
        assert_eq!(read_count(&key(path, 1)), 0, "and its headers with it");
    }

    #[test]
    fn a_light_is_read_only_when_no_header_is_known_where_its_file_is() {
        let light = |image_id: i32, path: &str| ProjectLight {
            image_id,
            path: PathBuf::from(path),
            night: "2026-09-10".into(),
            filter: "Ha".into(),
        };
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
                meta: readable.clone(),
                located: false,
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
