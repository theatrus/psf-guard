//! Calibration health of every project, worked out in the background and
//! kept below the database's cache folder.
//!
//! Judging a project reads every light's header and matches it against the
//! library (see [`crate::calibration::project_health`]), so no request ever
//! does it. A pass runs after the grading pass, which itself follows the
//! server start, imports, syncs, forgets and uploads; when a frame is chosen
//! for or against, a validity mark changes, or the calibration settings
//! change; when a directory refresh finishes or the image folders change;
//! and when a view finds a result behind the catalog, as after N.I.N.A.
//! writes new lights. It works out only the projects whose key changed: the
//! key covers the library and its grades, the project's lights, the
//! settings and the rules, and the pass also compares where the lights'
//! files were found. An unchanged project is never read again, and a
//! project judged again reads only the headers of lights whose file
//! changed.
//!
//! The Library reads the kept results only. One whose key no longer matches
//! is stale and is not shown until the next pass replaces it.

use crate::calibration::{
    HealthCode, HealthLevel, HealthLibrary, JudgedLight, KeptHeader, ProjectHealth, ProjectLight,
};
use crate::concurrency::{ComputePool, Priority};
use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, RwLock, Weak};
use std::time::{Duration, Instant};

/// The kept results, below the database's cache folder.
const CACHE_FILE: &str = "calibration-health.json";

/// The kept light headers, one file per project, below the database's cache
/// folder. Apart from the results, which every view reads.
const HEADERS_DIR: &str = "calibration-health-headers";

/// How often a pass looks again while interactive work runs.
const YIELD_POLL: Duration = Duration::from_millis(250);

/// How often the views may ask for a pass on one catalog. A change the
/// server makes asks for its own pass at once; the views catch the ones
/// nobody announces, such as N.I.N.A. writing lights.
const VIEW_ASK_INTERVAL: Duration = Duration::from_secs(120);

/// How long the views wait to ask again after a pass that failed.
const VIEW_ASK_AFTER_FAILURE: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Default, Serialize, Deserialize)]
struct HealthCache {
    version: u32,
    projects: BTreeMap<i32, CachedHealth>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedHealth {
    key: String,
    /// Where the project's lights were found when it was judged
    /// ([`crate::calibration::health_file_digests`]).
    #[serde(default)]
    files: String,
    computed_at: i64,
    health: ProjectHealth,
}

/// One project's light headers, by light id.
#[derive(Debug, Default, Serialize, Deserialize)]
struct KeptHeaders {
    /// The build that read them; another may read a header differently.
    version: String,
    lights: BTreeMap<i32, KeptHeader>,
}

fn cache_path(cache_dir: &FsPath) -> PathBuf {
    cache_dir.join(CACHE_FILE)
}

fn headers_path(cache_dir: &FsPath, project: i32) -> PathBuf {
    cache_dir.join(HEADERS_DIR).join(format!("{project}.json"))
}

/// One project's kept headers, or none when the file is missing, unreadable
/// or from another build.
fn read_headers(cache_dir: &FsPath, project: i32) -> BTreeMap<i32, KeptHeader> {
    std::fs::read(headers_path(cache_dir, project))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<KeptHeaders>(&bytes).ok())
        .filter(|kept| kept.version == env!("CARGO_PKG_VERSION"))
        .map(|kept| kept.lights)
        .unwrap_or_default()
}

/// Remove the kept headers of projects the catalog no longer holds,
/// whether or not the results file still lists them.
fn remove_gone_headers(cache_dir: &FsPath, keys: &BTreeMap<i32, String>) {
    let Ok(entries) = std::fs::read_dir(cache_dir.join(HEADERS_DIR)) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let project = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(|stem| stem.parse::<i32>().ok());
        if project.is_some_and(|project| !keys.contains_key(&project)) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

fn write_headers(
    cache_dir: &FsPath,
    project: i32,
    lights: BTreeMap<i32, KeptHeader>,
) -> anyhow::Result<()> {
    write_json(
        &headers_path(cache_dir, project),
        &KeptHeaders {
            version: env!("CARGO_PKG_VERSION").to_string(),
            lights,
        },
    )
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

fn write_cache(cache_dir: &FsPath, cache: &HealthCache) -> anyhow::Result<()> {
    write_json(&cache_path(cache_dir), cache)
}

/// Write a file whole under a temporary name, then rename it into place, so
/// a reader never sees half of it.
fn write_json(path: &FsPath, value: &impl Serialize) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, serde_json::to_vec(value)?)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
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
    /// The server that last asked, so a directory refresh can ask again.
    state: Mutex<Weak<AppState>>,
    work: Mutex<WorkDone>,
    views: Mutex<ViewAsks>,
}

/// When the views last asked for a pass, and when a pass last failed.
#[derive(Debug, Default, Clone, Copy)]
struct ViewAsks {
    asked: Option<Instant>,
    failed: Option<Instant>,
}

impl ViewAsks {
    /// Whether a view may ask for a pass now. Not while grading runs: it
    /// changes every key with each batch it commits, and asks for a pass
    /// when it ends. Not more often than [`VIEW_ASK_INTERVAL`], and not for
    /// [`VIEW_ASK_AFTER_FAILURE`] after a pass failed, so a pass that keeps
    /// failing does not run again for every poll.
    fn may_ask(&self, grading: bool, now: Instant) -> bool {
        let since = |at: Option<Instant>, wait: Duration| {
            at.is_none_or(|at| now.saturating_duration_since(at) >= wait)
        };
        !grading
            && since(self.asked, VIEW_ASK_INTERVAL)
            && since(self.failed, VIEW_ASK_AFTER_FAILURE)
    }
}

/// What the passes over one catalog have done since the server started.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct WorkDone {
    pub projects: usize,
    pub lights: usize,
    /// Light headers read from their files; the rest were kept from an
    /// earlier pass.
    pub headers_read: usize,
}

/// What the passes over the catalog at `path` have done since the server
/// started: the work a pass skipped shows here.
pub fn work_done(path: &str) -> WorkDone {
    *job(path).work.lock().unwrap_or_else(|p| p.into_inner())
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
    *job.state.lock().unwrap_or_else(|p| p.into_inner()) = Arc::downgrade(state);
    if !claim(&mut job.flags.lock().unwrap_or_else(|p| p.into_inner())) {
        return;
    }
    let state = Arc::clone(state);
    let path = path.to_string();
    // A stop waits for the light being judged; the project it cut short is
    // judged whole by the next pass.
    let work = state.shutdown.hold(format!("calibration health ({path})"));
    std::thread::spawn(move || {
        let _work = work;
        loop {
            let failed = run_pass(&state, &path, &job)
                .inspect_err(|error| {
                    tracing::info!(
                        "Paused working out calibration health in {path}: {error:#}. \
                         The next pass picks it up."
                    );
                })
                .is_err();
            job.views.lock().unwrap_or_else(|p| p.into_inner()).failed = failed.then(Instant::now);
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

/// A directory refresh of the catalog at `path` finished: a light's file
/// may have turned up, gone or moved. Only a catalog a server has asked
/// about is looked at again; the server start asks about each anyway.
pub fn tree_published(path: &str) {
    let state = JOBS
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(path)
        .and_then(|job| {
            job.state
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .upgrade()
        });
    if let Some(state) = state {
        request(&state, path);
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

/// One pass: judge each project whose key or lights' files changed,
/// writing the results after each so the Library sees them as they come.
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
    let tree = ctx.get_directory_tree()?;
    let mut files = crate::calibration::health_file_digests(&conn, &ctx.image_dirs, &tree)?;
    let mut cache = read_cache(&ctx.cache_dir_path);
    let known = cache.projects.len();
    cache
        .projects
        .retain(|project, _| keys.contains_key(project));
    remove_gone_headers(&ctx.cache_dir_path, &keys);
    let stale: Vec<(i32, String, String)> = keys
        .into_iter()
        .filter_map(|(project, key)| {
            let files = files.remove(&project).unwrap_or_default();
            let current = cache
                .projects
                .get(&project)
                .is_some_and(|kept| kept.key == key && kept.files == files);
            (!current).then_some((project, key, files))
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
    for (index, (project, key, files)) in stale.into_iter().enumerate() {
        if state.shutdown.is_stopping() {
            break;
        }
        let (lights, missing) =
            crate::calibration::project_lights(&conn, project, &tree, library.night_boundary())?;
        let judged = judge_project(
            &pool,
            &library,
            &lights,
            &read_headers(&ctx.cache_dir_path, project),
            &|| state.shutdown.is_stopping(),
            &|| state.interactive_job_active(),
        )?;
        let headers_read = judged.iter().filter(|light| light.read).count();
        let mut headers = BTreeMap::new();
        let mut results = Vec::with_capacity(judged.len());
        for light in judged {
            if let Some(header) = light.header {
                headers.insert(light.result.image_id, header);
            }
            results.push(light.result);
        }
        let health = crate::calibration::summarize_health(project, results, missing);
        if health.level == Some(HealthLevel::Poor) {
            poor += 1;
        }
        cache.projects.insert(
            project,
            CachedHealth {
                key,
                files,
                computed_at: chrono::Utc::now().timestamp(),
                health,
            },
        );
        write_cache(&ctx.cache_dir_path, &cache)?;
        // Losing the headers costs only reading them again.
        if let Err(error) = write_headers(&ctx.cache_dir_path, project, headers) {
            tracing::warn!("Could not keep the light headers of project {project}: {error:#}");
        }
        {
            let mut work = job.work.lock().unwrap_or_else(|p| p.into_inner());
            work.projects += 1;
            work.lights += lights.len();
            work.headers_read += headers_read;
        }
        job.progress.write().unwrap_or_else(|p| p.into_inner()).0 = index + 1;
    }
    if poor > 0 {
        tracing::info!("{poor} project(s) in {path} hold lights the library cannot calibrate");
    }
    Ok(())
}

/// Judge one project's lights on the pool, reusing each kept header whose
/// file has not changed. A light waits while `busy` says interactive work
/// runs, and once `stopping` says so the project ends between lights with
/// an error: nothing of it is kept.
fn judge_project(
    pool: &ComputePool,
    library: &HealthLibrary,
    lights: &[ProjectLight],
    kept: &BTreeMap<i32, KeptHeader>,
    stopping: &(dyn Fn() -> bool + Sync),
    busy: &(dyn Fn() -> bool + Sync),
) -> anyhow::Result<Vec<JudgedLight>> {
    pool.install(|| {
        lights
            .par_iter()
            .map(|light| {
                // Interactive work comes first; this light waits for it.
                while busy() && !stopping() {
                    std::thread::sleep(YIELD_POLL);
                }
                if stopping() {
                    anyhow::bail!("the server is stopping");
                }
                library.judge_light_kept(light, kept.get(&light.image_id))
            })
            .collect()
    })
}

/// The kept results whose keys still match the catalog, the only ones
/// shown, and whether some project has none that does.
fn fresh(path: &str, cache_dir: &FsPath) -> anyhow::Result<(BTreeMap<i32, CachedHealth>, bool)> {
    let conn = read_only(path)?;
    let keys = crate::calibration::health_keys(&conn)?;
    let projects = read_cache(cache_dir).projects;
    let behind = keys
        .iter()
        .any(|(project, key)| projects.get(project).is_none_or(|kept| &kept.key != key));
    let fresh = projects
        .into_iter()
        .filter(|(project, kept)| keys.get(project) == Some(&kept.key))
        .collect();
    Ok((fresh, behind))
}

/// Ask for a pass when a view finds a result behind the catalog. Nothing
/// else asks when N.I.N.A. writes new lights or the CLI regrades some; a
/// pass that runs already just runs once more. See [`ViewAsks::may_ask`]
/// for when a view holds back.
fn catch_up(state: &Arc<AppState>, ctx: &DbContext, behind: bool) {
    if !behind {
        return;
    }
    let path = &ctx.database_path;
    let grading = crate::server::calibration_quality::running(path);
    {
        let job = job(path);
        let mut views = job.views.lock().unwrap_or_else(|p| p.into_inner());
        let now = Instant::now();
        if !views.may_ask(grading, now) {
            return;
        }
        views.asked = Some(now);
    }
    request(state, path);
}

async fn fresh_for(
    state: &Arc<AppState>,
    ctx: &DbContext,
) -> Result<BTreeMap<i32, CachedHealth>, AppError> {
    let path = ctx.database_path.clone();
    let cache_dir = ctx.cache_dir_path.clone();
    let (fresh, behind) = tokio::task::spawn_blocking(move || fresh(&path, &cache_dir))
        .await
        .map_err(|error| AppError::InternalError(format!("reading calibration health: {error}")))?
        .map_err(|error| {
            AppError::InternalError(format!("reading calibration health: {error:#}"))
        })?;
    catch_up(state, ctx, behind);
    Ok(fresh)
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
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<CalibrationHealthResponse>>, AppError> {
    let projects = fresh_for(&state, &ctx)
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
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, project_id)): Path<(String, i32)>,
) -> Result<Json<ApiResponse<ProjectHealthResponse>>, AppError> {
    let kept = fresh_for(&state, &ctx).await?.remove(&project_id);
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
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Query(query): Query<LightMarksQuery>,
) -> Result<Json<ApiResponse<LightMarksResponse>>, AppError> {
    let path = ctx.database_path.clone();
    let cache_dir = ctx.cache_dir_path.clone();
    let (lights, stale_projects, behind) = tokio::task::spawn_blocking(move || {
        let conn = read_only(&path)?;
        let keys = crate::calibration::health_keys(&conn)?;
        let projects = read_cache(&cache_dir).projects;
        let behind = keys
            .iter()
            .any(|(project, key)| projects.get(project).is_none_or(|kept| &kept.key != key));
        let mut lights = Vec::new();
        let mut stale = Vec::new();
        for (project, kept) in projects {
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
        anyhow::Ok((lights, stale, behind))
    })
    .await
    .map_err(|error| AppError::InternalError(format!("reading calibration marks: {error}")))?
    .map_err(|error| AppError::InternalError(format!("reading calibration marks: {error:#}")))?;
    catch_up(&state, &ctx, behind);
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
    fn a_stop_ends_a_project_between_lights() {
        let conn = Connection::open_in_memory().unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        let library = HealthLibrary::load(&conn).unwrap();
        let lights: Vec<ProjectLight> = (0..200)
            .map(|image_id| ProjectLight {
                image_id,
                path: PathBuf::from(format!("light-{image_id}.fits")),
                night: "2026-09-09".into(),
                filter: "Ha".into(),
            })
            .collect();
        let pool = ComputePool::take("calibration-health-test", 1).unwrap();
        let none = BTreeMap::new();
        let judged = judge_project(&pool, &library, &lights, &none, &|| false, &|| false).unwrap();
        assert_eq!(judged.len(), lights.len());

        // A stop that comes after five lights ends the project there.
        let asked = AtomicUsize::new(0);
        let stopping = || asked.fetch_add(1, Ordering::SeqCst) >= 5;
        let stopped = judge_project(&pool, &library, &lights, &none, &stopping, &|| false);
        assert!(stopped.is_err());
        assert!(asked.load(Ordering::SeqCst) < 20, "{asked:?}");
    }

    #[test]
    fn headers_of_projects_gone_from_the_catalog_are_removed() {
        let temp = tempfile::tempdir().unwrap();
        for project in [1, 2] {
            write_headers(temp.path(), project, BTreeMap::new()).unwrap();
        }
        // Results from other rules are not even read; the headers of the
        // project that went still go.
        let keys = BTreeMap::from([(1, "k".to_string())]);
        remove_gone_headers(temp.path(), &keys);
        assert!(headers_path(temp.path(), 1).is_file());
        assert!(!headers_path(temp.path(), 2).exists());
    }

    #[test]
    fn views_hold_back_while_grading_runs_and_after_asking_or_failing() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut views = ViewAsks::default();
        assert!(views.may_ask(false, start));
        assert!(!views.may_ask(true, start), "grading asks when it ends");

        // Once asked, the views wait their interval.
        views.asked = Some(start);
        assert!(!views.may_ask(false, at(60)));
        assert!(views.may_ask(false, at(VIEW_ASK_INTERVAL.as_secs())));

        // A pass that failed holds them back longer.
        views.failed = Some(at(10));
        assert!(!views.may_ask(false, at(VIEW_ASK_INTERVAL.as_secs() + 10)));
        assert!(views.may_ask(false, at(VIEW_ASK_AFTER_FAILURE.as_secs() + 10)));
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
                files: String::new(),
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
