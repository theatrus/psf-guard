//! The Calibration page's project scope: the sessions, masters and flat
//! groups one project's lights use (see
//! [`crate::calibration::project_scope`]).
//!
//! Working it out reads each light's header and matches it against the
//! library. The headers come from those the calibration health pass keeps,
//! so only a light the pass has not reached is read here. It runs on a
//! read-only connection of its own off the async workers, never on the
//! shared request connection, and is kept in memory per catalog revision:
//! the project's health key (the library, its grades and flat groups, the
//! settings and the project's lights) with the kept headers' file stamp.

use crate::calibration::project_scope::{self, ProjectScope};
use crate::calibration::KeptHeader;
use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use axum::extract::Query;
use axum::Json;
use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::UNIX_EPOCH;

/// Scopes kept at most, across every catalog; the oldest go first.
const KEPT_SCOPES: usize = 32;

/// What one kept scope was worked out from.
type Revision = String;

#[derive(Default)]
struct Kept {
    /// By catalog path and project, with its revision and when it was used.
    scopes: HashMap<(String, i32), (Revision, u64, Arc<ProjectScope>)>,
    clock: u64,
}

static KEPT: LazyLock<Mutex<Kept>> = LazyLock::new(Mutex::default);

/// One lock per catalog and project, so the page's lists, asked at once,
/// share one working out instead of each doing it.
static WORKING: LazyLock<Mutex<HashMap<ScopeKey, Arc<Mutex<()>>>>> = LazyLock::new(Mutex::default);

/// A catalog's path and a project.
type ScopeKey = (String, i32);

fn working_lock(path: &str, project: i32) -> Arc<Mutex<()>> {
    let mut locks = WORKING.lock().unwrap_or_else(|p| p.into_inner());
    // Locks no one holds or waits for go, so the map stays small.
    locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    Arc::clone(locks.entry((path.to_string(), project)).or_default())
}

fn read_only(path: &str) -> anyhow::Result<Connection> {
    Ok(
        crate::server::database_context::open_scheduler_connection_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?,
    )
}

/// The catalog revision a project's scope depends on.
fn revision(conn: &Connection, ctx: &DatabaseContext, project: i32) -> anyhow::Result<Revision> {
    let key = crate::calibration::health_keys(conn)?
        .remove(&project)
        .unwrap_or_default();
    let headers = crate::server::calibration_health::headers_stamp(&ctx.cache_dir_path, project);
    Ok(format!("{key}:{headers}"))
}

fn kept(path: &str, project: i32, revision: &str) -> Option<Arc<ProjectScope>> {
    let mut kept = KEPT.lock().unwrap_or_else(|p| p.into_inner());
    kept.clock += 1;
    let now = kept.clock;
    let entry = kept.scopes.get_mut(&(path.to_string(), project))?;
    if entry.0 != revision {
        return None;
    }
    entry.1 = now;
    Some(Arc::clone(&entry.2))
}

fn keep(path: &str, project: i32, revision: Revision, scope: Arc<ProjectScope>) {
    let mut kept = KEPT.lock().unwrap_or_else(|p| p.into_inner());
    kept.clock += 1;
    let now = kept.clock;
    kept.scopes
        .insert((path.to_string(), project), (revision, now, scope));
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
    }
}

/// The headers of a project's lights: the kept one when its file is where
/// the pass found it, else read now. Unreadable lights are counted.
fn light_headers(
    lights: &[crate::calibration::ProjectLight],
    kept: &BTreeMap<i32, KeptHeader>,
) -> (Vec<crate::commands::import::headers::FrameMeta>, usize) {
    let mut metas = Vec::with_capacity(lights.len());
    let mut unread = 0;
    for light in lights {
        let meta = match kept.get(&light.image_id) {
            Some(header) if header.path == light.path => header.meta.clone(),
            _ => crate::commands::import::headers::read_frame_meta(&light.path),
        };
        if meta.readable {
            metas.push(meta);
        } else {
            unread += 1;
        }
    }
    (metas, unread)
}

/// Work out one project's scope, or take the kept one. Blocking: call it
/// off the async workers.
pub(crate) fn scope_of(ctx: &DatabaseContext, project: i32) -> anyhow::Result<Arc<ProjectScope>> {
    let path = ctx.database_path.as_str();
    let conn = read_only(path)?;
    let revision = revision(&conn, ctx, project)?;
    if let Some(scope) = kept(path, project, &revision) {
        return Ok(scope);
    }
    let lock = working_lock(path, project);
    let _working = lock.lock().unwrap_or_else(|p| p.into_inner());
    // Another request may have worked it out while this one waited.
    if let Some(scope) = kept(path, project, &revision) {
        return Ok(scope);
    }
    let tree = ctx.get_directory_tree()?;
    let boundary = crate::calibration::catalog_night_boundary(&conn);
    let (lights, missing) = crate::calibration::project_lights(&conn, project, &tree, boundary)?;
    let headers = crate::server::calibration_health::kept_headers(&ctx.cache_dir_path, project);
    let (metas, unread) = light_headers(&lights, &headers);
    let scope = Arc::new(project_scope::project_scope(
        &conn,
        project,
        &metas,
        missing + unread,
    )?);
    keep(path, project, revision, Arc::clone(&scope));
    Ok(scope)
}

/// A project's scope for a request, worked out off the async workers.
pub(crate) async fn scope_for(
    ctx: &Arc<DatabaseContext>,
    project: i32,
) -> Result<Arc<ProjectScope>, AppError> {
    let ctx = Arc::clone(ctx);
    tokio::task::spawn_blocking(move || scope_of(&ctx, project))
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
/// project's lights use from the library: its sessions, masters and rigs.
pub async fn get_project_scope(
    ctx: DbContext,
    Query(query): Query<ScopeQuery>,
) -> Result<Json<ApiResponse<ProjectScope>>, AppError> {
    let scope = scope_for(&ctx.0, query.project).await?;
    Ok(Json(ApiResponse::success((*scope).clone())))
}

/// When the file was last modified, in nanoseconds, or 0 when it is not
/// there.
pub(crate) fn file_stamp(path: &std::path::Path) -> u128 {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_scope_goes_first_and_another_revision_misses() {
        let path = "kept-scope-test.sqlite";
        for project in 0..KEPT_SCOPES as i32 {
            keep(path, project, "r".into(), Arc::default());
        }
        assert!(kept(path, 0, "r").is_some(), "used again, so kept longest");
        keep(path, 999, "r".into(), Arc::default());
        assert!(kept(path, 0, "r").is_some());
        assert!(kept(path, 1, "r").is_none(), "the least recently used went");
        assert!(
            kept(path, 0, "s").is_none(),
            "another revision is not served"
        );
    }
}
