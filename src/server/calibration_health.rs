//! Calibration health of every project, worked out in the background and
//! kept below the database's cache folder.
//!
//! Judging a project reads every light's header and matches it against the
//! library (see [`crate::calibration::project_health`]), so no request ever
//! does it. A pass runs after the grading pass, which itself follows the
//! server start, imports, syncs, forgets and uploads; and when a frame is
//! chosen for or against, a validity mark changes, or the calibration
//! settings change. It works out only the projects whose key changed: the
//! key covers the library and its grades, the project's lights, the
//! settings and the rules, so an unchanged project is never read again.
//!
//! The Library reads the kept results only. One whose key no longer matches
//! is stale and is not shown until the next pass replaces it.

use crate::calibration::{HealthCode, HealthLevel, HealthLibrary, ProjectHealth};
use crate::concurrency::{ComputePool, Priority};
use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;
use axum::extract::{Path, Query};
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock};
use std::time::Duration;

/// The kept results, below the database's cache folder.
const CACHE_FILE: &str = "calibration-health.json";

/// How often a pass looks again while interactive work runs.
const YIELD_POLL: Duration = Duration::from_millis(250);

#[derive(Debug, Default, Serialize, Deserialize)]
struct HealthCache {
    version: u32,
    projects: BTreeMap<i32, CachedHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedHealth {
    key: String,
    computed_at: i64,
    health: ProjectHealth,
}

fn cache_path(cache_dir: &FsPath) -> PathBuf {
    cache_dir.join(CACHE_FILE)
}

/// The kept results, or none when the file is missing, unreadable, or from
/// other rules.
fn read_cache(cache_dir: &FsPath) -> HealthCache {
    std::fs::read(cache_path(cache_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<HealthCache>(&bytes).ok())
        .filter(|cache| cache.version == crate::calibration::HEALTH_ALGORITHM_VERSION)
        .unwrap_or_default()
}

/// Distinguishes the temporary files of one process's writes.
static WRITES: AtomicUsize = AtomicUsize::new(0);

/// Write the results whole under a temporary name, then rename it into
/// place, so a reader never sees half a file.
fn write_cache(cache_dir: &FsPath, cache: &HealthCache) -> anyhow::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let path = cache_path(cache_dir);
    let temporary = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, serde_json::to_vec(cache)?)?;
    if let Err(error) = std::fs::rename(&temporary, &path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Whether a pass runs, and whether another was asked for meanwhile.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct Flags {
    running: bool,
    again: bool,
}

fn claim(flags: &mut Flags) -> bool {
    if flags.running {
        flags.again = true;
        false
    } else {
        flags.running = true;
        true
    }
}

fn finish(flags: &mut Flags) -> bool {
    if flags.again {
        flags.again = false;
        true
    } else {
        flags.running = false;
        false
    }
}

#[derive(Default)]
struct Job {
    flags: Mutex<Flags>,
    /// Projects judged so far this pass, and how many it set out to judge.
    progress: RwLock<(usize, usize)>,
}

/// One job per catalog file, whichever database context asks.
static JOBS: LazyLock<Mutex<HashMap<String, Arc<Job>>>> = LazyLock::new(Default::default);

fn job(path: &str) -> Arc<Job> {
    Arc::clone(
        JOBS.lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .entry(path.to_string())
            .or_default(),
    )
}

fn running(path: &str) -> bool {
    job(path)
        .flags
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .running
}

/// Work out the health of every project in the catalog at `path` whose key
/// changed, on a thread of its own. A request while a pass runs makes it
/// run once more when it ends.
pub fn request(state: &Arc<AppState>, path: &str) {
    if state.shutdown.is_stopping() {
        return;
    }
    let job = job(path);
    if !claim(&mut job.flags.lock().unwrap_or_else(|p| p.into_inner())) {
        return;
    }
    let state = Arc::clone(state);
    let path = path.to_string();
    // A stop waits for the project being judged to be written.
    let work = state.shutdown.hold(format!("calibration health ({path})"));
    std::thread::spawn(move || {
        let _work = work;
        loop {
            if let Err(error) = run_pass(&state, &path, &job) {
                tracing::info!(
                    "Paused working out calibration health in {path}: {error:#}. \
                     The next pass picks it up."
                );
            }
            let mut flags = job.flags.lock().unwrap_or_else(|p| p.into_inner());
            if state.shutdown.is_stopping() {
                // Let go of the catalog: the next start judges the rest.
                flags.running = false;
                flags.again = false;
                break;
            }
            if !finish(&mut flags) {
                break;
            }
        }
    });
}

/// Every database's projects, after the calibration settings changed.
pub fn request_all(state: &Arc<AppState>) {
    for ctx in state.all_databases() {
        request(state, &ctx.database_path);
    }
}

fn read_only(path: &str) -> anyhow::Result<Connection> {
    Ok(
        crate::server::database_context::open_scheduler_connection_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?,
    )
}

/// One pass: judge each project whose key changed, writing the results
/// after each so the Library sees them as they come.
fn run_pass(state: &AppState, path: &str, job: &Job) -> anyhow::Result<()> {
    let Some(ctx) = state
        .all_databases()
        .into_iter()
        .find(|ctx| ctx.database_path == path)
    else {
        return Ok(());
    };
    let conn = read_only(path)?;
    let keys = crate::calibration::health_keys(&conn)?;
    let mut cache = read_cache(&ctx.cache_dir_path);
    let known = cache.projects.len();
    cache
        .projects
        .retain(|project, _| keys.contains_key(project));
    let stale: Vec<(i32, String)> = keys
        .into_iter()
        .filter(|(project, key)| {
            cache
                .projects
                .get(project)
                .is_none_or(|kept| &kept.key != key)
        })
        .collect();
    cache.version = crate::calibration::HEALTH_ALGORITHM_VERSION;
    if stale.is_empty() {
        if cache.projects.len() != known {
            write_cache(&ctx.cache_dir_path, &cache)?;
        }
        return Ok(());
    }
    *job.progress.write().unwrap_or_else(|p| p.into_inner()) = (0, stale.len());
    let library = HealthLibrary::load(&conn)?;
    let tree = ctx.get_directory_tree()?;
    // Header reads, mostly: little memory, so the core budget decides.
    let budget =
        crate::concurrency::plan_workers(None, &state.worker_policy(), Priority::Background, None);
    let lease = state.lease_workers(Priority::Background, budget.workers);
    let pool = ComputePool::take("calibration-health", lease.workers)?;
    tracing::info!(
        "Working out calibration health for {} project(s) in {path} — {}",
        stale.len(),
        lease.summary()
    );
    let mut poor = 0;
    for (index, (project, key)) in stale.into_iter().enumerate() {
        if state.shutdown.is_stopping() {
            break;
        }
        let (lights, missing) =
            crate::calibration::project_lights(&conn, project, &tree, library.night_boundary())?;
        let results = pool.install(|| {
            lights
                .par_iter()
                .map(|light| {
                    // Interactive work comes first; this light waits for it.
                    while state.interactive_job_active() && !state.shutdown.is_stopping() {
                        std::thread::sleep(YIELD_POLL);
                    }
                    library.judge_light(light)
                })
                .collect::<anyhow::Result<Vec<_>>>()
        })?;
        let health = crate::calibration::summarize_health(project, results, missing);
        if health.level == Some(HealthLevel::Poor) {
            poor += 1;
        }
        cache.projects.insert(
            project,
            CachedHealth {
                key,
                computed_at: chrono::Utc::now().timestamp(),
                health,
            },
        );
        write_cache(&ctx.cache_dir_path, &cache)?;
        job.progress.write().unwrap_or_else(|p| p.into_inner()).0 = index + 1;
    }
    if poor > 0 {
        tracing::info!("{poor} project(s) in {path} hold lights the library cannot calibrate");
    }
    Ok(())
}

/// The kept results whose keys still match the catalog: the only ones
/// shown.
fn fresh(path: &str, cache_dir: &FsPath) -> anyhow::Result<BTreeMap<i32, CachedHealth>> {
    let conn = read_only(path)?;
    let keys = crate::calibration::health_keys(&conn)?;
    Ok(read_cache(cache_dir)
        .projects
        .into_iter()
        .filter(|(project, kept)| keys.get(project) == Some(&kept.key))
        .collect())
}

async fn fresh_for(ctx: &DbContext) -> Result<BTreeMap<i32, CachedHealth>, AppError> {
    let path = ctx.database_path.clone();
    let cache_dir = ctx.cache_dir_path.clone();
    tokio::task::spawn_blocking(move || fresh(&path, &cache_dir))
        .await
        .map_err(|error| AppError::InternalError(format!("reading calibration health: {error}")))?
        .map_err(|error| AppError::InternalError(format!("reading calibration health: {error:#}")))
}

/// One project in the Library's list.
#[derive(Debug, Serialize)]
pub struct ProjectHealthSummary {
    pub project_id: i32,
    pub level: Option<HealthLevel>,
    pub poor_lights: usize,
    pub check_lights: usize,
    pub checked: usize,
    pub computed_at: i64,
}

#[derive(Debug, Serialize)]
pub struct CalibrationHealthResponse {
    /// Projects with a current result; one missing has none yet.
    pub projects: Vec<ProjectHealthSummary>,
    /// A pass is working out results now.
    pub running: bool,
}

/// `GET /api/db/{db}/calibration-health` — each project's level and counts,
/// from the kept results.
pub async fn get_health(
    ctx: DbContext,
) -> Result<Json<ApiResponse<CalibrationHealthResponse>>, AppError> {
    let projects = fresh_for(&ctx)
        .await?
        .into_values()
        .map(|kept| ProjectHealthSummary {
            project_id: kept.health.project_id,
            level: kept.health.level,
            poor_lights: kept.health.poor_lights,
            check_lights: kept.health.check_lights,
            checked: kept.health.checked,
            computed_at: kept.computed_at,
        })
        .collect();
    Ok(Json(ApiResponse::success(CalibrationHealthResponse {
        projects,
        running: running(&ctx.database_path),
    })))
}

#[derive(Debug, Serialize)]
pub struct ProjectHealthResponse {
    /// Absent until a pass has judged the project as it is now.
    pub health: Option<ProjectHealth>,
    pub computed_at: Option<i64>,
    pub running: bool,
}

/// `GET /api/db/{db}/projects/{project_id}/calibration-health` — one
/// project's findings, by night and filter.
pub async fn get_project_health(
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ProjectHealthResponse>>, AppError> {
    let kept = fresh_for(&ctx).await?.remove(&project_id);
    Ok(Json(ApiResponse::success(ProjectHealthResponse {
        computed_at: kept.as_ref().map(|kept| kept.computed_at),
        health: kept.map(|kept| kept.health),
        running: running(&ctx.database_path),
    })))
}

/// One light with a finding, as the image views mark it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct LightMark {
    pub image_id: i32,
    /// Its worst finding's level.
    pub level: HealthLevel,
    /// Every rule that found something in it.
    pub codes: Vec<HealthCode>,
}

/// Each light of a project's result that has a finding, by id.
fn marks_of(health: &ProjectHealth) -> Vec<LightMark> {
    let mut lights: BTreeMap<i32, (HealthLevel, BTreeSet<HealthCode>)> = BTreeMap::new();
    for finding in &health.findings {
        for &image_id in &finding.image_ids {
            let entry = lights
                .entry(image_id)
                .or_insert((finding.level, BTreeSet::new()));
            entry.0 = entry.0.max(finding.level);
            entry.1.insert(finding.code);
        }
    }
    lights
        .into_iter()
        .map(|(image_id, (level, codes))| LightMark {
            image_id,
            level,
            codes: codes.into_iter().collect(),
        })
        .collect()
}

#[derive(Debug, Deserialize)]
pub struct LightMarksQuery {
    /// One project's lights; every project's when absent.
    pub project_id: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct LightMarksResponse {
    /// Lights with a finding, by id. A light not listed has none, or its
    /// project has no result yet.
    pub lights: Vec<LightMark>,
    /// Projects whose result came before a change: a light rejected or
    /// imported, or the library changed. The next pass replaces it.
    pub stale_projects: Vec<i32>,
    /// A pass is working out results now.
    pub running: bool,
}

/// `GET /api/db/{db}/calibration-health/lights` — the lights each kept
/// result found something in, for the image views to mark and filter.
/// Unlike the Library, it serves a result a change has made stale, and says
/// so: rejecting lights would otherwise blank every mark in their project
/// until the next pass.
pub async fn get_light_marks(
    ctx: DbContext,
    Query(query): Query<LightMarksQuery>,
) -> Result<Json<ApiResponse<LightMarksResponse>>, AppError> {
    let path = ctx.database_path.clone();
    let cache_dir = ctx.cache_dir_path.clone();
    let (lights, stale_projects) = tokio::task::spawn_blocking(move || {
        let conn = read_only(&path)?;
        let keys = crate::calibration::health_keys(&conn)?;
        let mut lights = Vec::new();
        let mut stale = Vec::new();
        for (project, kept) in read_cache(&cache_dir).projects {
            if query.project_id.is_some_and(|wanted| wanted != project) {
                continue;
            }
            match keys.get(&project) {
                None => continue,
                Some(key) if key != &kept.key => stale.push(project),
                Some(_) => {}
            }
            lights.extend(marks_of(&kept.health));
        }
        anyhow::Ok((lights, stale))
    })
    .await
    .map_err(|error| AppError::InternalError(format!("reading calibration marks: {error}")))?
    .map_err(|error| AppError::InternalError(format!("reading calibration marks: {error:#}")))?;
    Ok(Json(ApiResponse::success(LightMarksResponse {
        lights,
        stale_projects,
        running: running(&ctx.database_path),
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_light_takes_its_worst_level_and_every_code() {
        use crate::calibration::HealthFinding;
        let finding = |code, level, image_ids: Vec<i32>| HealthFinding {
            code,
            level,
            reason: String::new(),
            lights: image_ids.len(),
            nights: Vec::new(),
            image_ids,
        };
        let health = ProjectHealth {
            project_id: 1,
            level: Some(HealthLevel::Poor),
            checked: 3,
            missing_files: 0,
            poor_lights: 1,
            check_lights: 1,
            findings: vec![
                finding(HealthCode::CannotCalibrate, HealthLevel::Poor, vec![4]),
                finding(HealthCode::OldFlats, HealthLevel::Check, vec![2, 4]),
            ],
        };
        assert_eq!(
            marks_of(&health),
            vec![
                LightMark {
                    image_id: 2,
                    level: HealthLevel::Check,
                    codes: vec![HealthCode::OldFlats],
                },
                LightMark {
                    image_id: 4,
                    level: HealthLevel::Poor,
                    codes: vec![HealthCode::CannotCalibrate, HealthCode::OldFlats],
                },
            ]
        );
    }

    #[test]
    fn a_request_during_a_pass_runs_one_more() {
        let mut flags = Flags::default();
        assert!(claim(&mut flags));
        assert!(!claim(&mut flags));
        assert!(finish(&mut flags));
        assert!(!finish(&mut flags));
        assert_eq!(flags, Flags::default());
    }

    #[test]
    fn the_cache_is_written_whole_and_read_back_by_its_rules() {
        let temp = tempfile::tempdir().unwrap();
        assert!(read_cache(temp.path()).projects.is_empty(), "no file yet");
        let health = crate::calibration::summarize_health(3, Vec::new(), 2);
        let mut cache = HealthCache {
            version: crate::calibration::HEALTH_ALGORITHM_VERSION,
            projects: BTreeMap::new(),
        };
        cache.projects.insert(
            3,
            CachedHealth {
                key: "k".into(),
                computed_at: 1,
                health: health.clone(),
            },
        );
        write_cache(temp.path(), &cache).unwrap();
        let entries: Vec<_> = std::fs::read_dir(temp.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(entries, [CACHE_FILE], "no temporary file is left");
        assert_eq!(read_cache(temp.path()).projects[&3].health, health);

        // Results from other rules, or a broken file, read as none.
        cache.version += 1;
        write_cache(temp.path(), &cache).unwrap();
        assert!(read_cache(temp.path()).projects.is_empty());
        std::fs::write(cache_path(temp.path()), b"{").unwrap();
        assert!(read_cache(temp.path()).projects.is_empty());
    }
}
