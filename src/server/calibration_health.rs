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
use crate::concurrency::{ComputePool, Priority, Stop};
use crate::server::api::ApiResponse;
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::job_line::catalog::{CatalogJob, CatalogWork, Step};
use crate::server::state::AppState;
use axum::extract::{Path, Query, State};
use axum::Json;
use rayon::prelude::*;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path as FsPath, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The kept results, below the database's cache folder.
const CACHE_FILE: &str = "calibration-health.json";

/// Where builds before the catalog kept light headers wrote them, one file
/// per project, below the database's cache folder. A pass removes it.
const OLD_HEADERS_DIR: &str = "calibration-health-headers";

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

fn cache_path(cache_dir: &FsPath) -> PathBuf {
    cache_dir.join(CACHE_FILE)
}

/// Keep the headers a pass read in the catalog, on a writable connection
/// opened for the first project that read any. The pass itself reads on a
/// read-only one.
fn keep_headers(
    writer: &mut Option<Connection>,
    path: &str,
    headers: &BTreeMap<i32, KeptHeader>,
) -> anyhow::Result<()> {
    if headers.is_empty() {
        return Ok(());
    }
    let conn = match writer {
        Some(conn) => conn,
        None => writer.insert(
            crate::server::database_context::open_scheduler_connection_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
            )?,
        ),
    };
    crate::light_headers::store(conn, headers.iter().map(|(id, header)| (*id, header)))?;
    Ok(())
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
    write_bytes(path, &serde_json::to_vec(value)?)
}

/// Write `bytes` to a temporary file beside `path` and rename it over.
fn write_bytes(path: &FsPath, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "json.tmp-{}-{}",
        std::process::id(),
        WRITES.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::write(&temporary, bytes)?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

/// Calibration health passes, run on one thread per catalog.
pub(crate) struct CalibrationHealth;

type Job = CatalogJob<CalibrationHealth>;

#[derive(Debug, Default)]
pub(crate) struct Queue {
    /// Whether a pass was requested after the last pass started.
    pass_requested: bool,
    views: ViewAsks,
}

/// The progress of the current pass, and totals since the server started.
#[derive(Debug, Default, Clone)]
pub(crate) struct HealthStatus {
    /// Projects judged so far in this pass, and the number it will judge.
    progress: (usize, usize),
    work: WorkDone,
}

impl CatalogWork for CalibrationHealth {
    type Queue = Queue;
    type Status = HealthStatus;
    type Item = ();
    const RUNNER: &'static str = "calibration-health";
    /// Shutdown waits for the light being judged. The next pass judges
    /// again the whole project that shutdown interrupted.
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("calibration health");

    /// A request during a pass starts one more pass after it ends.
    fn next(_: &AppState, _: &str, queue: &mut Queue, _: Instant) -> Step<()> {
        if std::mem::take(&mut queue.pass_requested) {
            Step::Run(())
        } else {
            Step::Exit
        }
    }

    fn run(state: &Arc<AppState>, path: &str, job: &Job, (): (), _: &Stop) {
        // The count is the new pass's from its start: a pass with nothing
        // to judge shows no progress from an earlier one.
        job.update(|status| status.progress = (0, 0));
        let failed = run_pass(state, path, job)
            .inspect_err(|error| {
                tracing::info!(
                    "Paused working out calibration health in {path}: {error:#}. \
                     The next pass picks it up."
                );
            })
            .is_err();
        job.queue().views.failed = failed.then(Instant::now);
    }

    fn has_work(queue: &Queue) -> bool {
        queue.pass_requested
    }

    fn runner_failed(_: &mut Queue, _: &mut HealthStatus) {}
}

/// When a view last requested a pass, and when a pass last failed.
#[derive(Debug, Default, Clone, Copy)]
struct ViewAsks {
    requested_at: Option<Instant>,
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
            && since(self.requested_at, VIEW_ASK_INTERVAL)
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
pub fn work_done(state: &AppState, path: &str) -> WorkDone {
    job(state, path).status().work
}

/// How far a pass over a catalog has got, for its row in the job list.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HealthProgress {
    /// Whether a pass runs or is queued.
    pub running: bool,
    /// Projects judged so far in this pass.
    pub judged: usize,
    /// Projects whose health this pass works out again; 0 until the pass
    /// has found them, and when it finds none.
    pub total: usize,
}

/// How far the pass over the catalog at `path` has got.
pub fn progress(state: &AppState, path: &str) -> HealthProgress {
    let (judged, total) = job(state, path).status().progress;
    HealthProgress {
        running: running(state, path),
        judged,
        total,
    }
}

/// The job of the catalog file at `path`, whichever database context asks.
fn job(state: &AppState, path: &str) -> Arc<Job> {
    state.catalog_jobs.calibration_health.job(path)
}

fn running(state: &AppState, path: &str) -> bool {
    job(state, path).queue().has_runner()
}

/// Work out the health of every project in the catalog at `path` whose key
/// changed, on a thread of its own. A request while a pass runs makes it
/// run once more when it ends.
pub fn request(state: &Arc<AppState>, path: &str) {
    // A directory refresh of the catalog then asks this server again.
    state.link_contexts(path);
    state
        .catalog_jobs
        .calibration_health
        .request(state, path, |queue| queue.pass_requested = true);
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
pub fn tree_published(state: &Arc<AppState>, path: &str) {
    if state
        .catalog_jobs
        .calibration_health
        .existing(path)
        .is_some()
    {
        request(state, path);
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
    tidy_headers(&conn, path);
    let keys = crate::calibration::health_keys(&conn)?;
    let tree = ctx.get_directory_tree()?;
    let mut files = crate::calibration::health_file_digests(&conn, &ctx.image_dirs, &tree)?;
    let mut cache = read_cache(&ctx.cache_dir_path);
    let known = cache.projects.len();
    cache
        .projects
        .retain(|project, _| keys.contains_key(project));
    // Headers live in the catalog now; an older build's files go.
    let _ = std::fs::remove_dir_all(ctx.cache_dir_path.join(OLD_HEADERS_DIR));
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
    job.update(|status| status.progress = (0, stale.len()));
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
    let mut writer: Option<Connection> = None;
    for (index, (project, key, files)) in stale.into_iter().enumerate() {
        if state.shutdown.is_stopping() {
            break;
        }
        let (lights, missing) = crate::calibration::project_lights(
            &conn,
            project,
            &tree,
            &library.light_nights(&conn),
        )?;
        let kept = crate::light_headers::project_headers(&conn, project).unwrap_or_else(|error| {
            tracing::debug!("kept headers of project {project} unavailable: {error:#}");
            BTreeMap::new()
        });
        let judged = judge_project(
            &pool,
            &library,
            &lights,
            &kept,
            &|| state.shutdown.is_stopping(),
            &|| state.interactive_job_active(),
        )?;
        let headers_read = judged.iter().filter(|light| light.read).count();
        let mut headers = BTreeMap::new();
        let mut results = Vec::with_capacity(judged.len());
        for light in judged {
            // Only what was read: a kept header stands as it is.
            if light.read
                && let Some(header) = light.header
            {
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
        if let Err(error) = keep_headers(&mut writer, path, &headers) {
            tracing::warn!("Could not keep the light headers of project {project}: {error:#}");
        }
        job.update(|status| {
            status.work.projects += 1;
            status.work.lights += lights.len();
            status.work.headers_read += headers_read;
            status.progress.0 = index + 1;
        });
    }
    if poor > 0 {
        tracing::info!("{poor} project(s) in {path} hold lights the library cannot calibrate");
    }
    Ok(())
}

/// Before each pass, whether or not it reads a header: keep what stack
/// preparation read and nobody has written yet, and forget the headers of
/// lights the catalog no longer holds, which `conn`, read-only, finds first
/// so a catalog with none is not opened for writing.
fn tidy_headers(conn: &Connection, path: &str) {
    crate::server::light_header_writes::write_pending(path);
    if !crate::light_headers::has_orphans(conn) {
        return;
    }
    let pruned = crate::server::database_context::open_scheduler_connection_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(anyhow::Error::from)
    .and_then(|writer| crate::light_headers::prune(&writer));
    if let Err(error) = pruned {
        tracing::debug!("Could not forget the headers of removed lights in {path}: {error:#}");
    }
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
    let grading = crate::server::calibration_quality::running(state, path);
    {
        let job = job(state, path);
        let mut queue = job.queue();
        let now = Instant::now();
        if !queue.views.may_ask(grading, now) {
            return;
        }
        queue.views.requested_at = Some(now);
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
        running: running(&state, &ctx.database_path),
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
        running: running(&state, &ctx.database_path),
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
        running: running(&state, &ctx.database_path),
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
                image_guid: None,
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

    /// Background grading moves every project's health key, so the pass
    /// judges each project again. It keeps only the headers it read, in the
    /// catalog: the same headers kept again keep their stamp, which the
    /// project scope and what the flat groups feed are worked out from; new
    /// ones move it.
    #[test]
    fn a_pass_keeps_the_headers_it_read_in_the_catalog() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = temp.path().join("catalog.sqlite");
        let conn = Connection::open(&catalog).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata, guid)
             VALUES (7, 1, 1, 0, 'Ha', 0, '{}', 'light-7')",
            [],
        )
        .unwrap();
        let path = catalog.to_string_lossy().into_owned();
        let mut writer = None;
        keep_headers(&mut writer, &path, &BTreeMap::new()).unwrap();
        assert!(writer.is_none(), "nothing read, nothing opened");
        assert_eq!(crate::light_headers::project_stamp(&conn, 1), 0);

        let light = temp.path().join("light.fits");
        std::fs::write(&light, b"bytes").unwrap();
        let header = KeptHeader::new(
            &light,
            crate::light_headers::FileStamp::of(&light).unwrap(),
            crate::commands::import::headers::FrameMeta::default(),
        );
        keep_headers(&mut writer, &path, &BTreeMap::from([(7, header.clone())])).unwrap();
        let before = crate::light_headers::project_stamp(&conn, 1);
        assert_ne!(before, 0);
        keep_headers(&mut writer, &path, &BTreeMap::from([(7, header)])).unwrap();
        assert_eq!(crate::light_headers::project_stamp(&conn, 1), before);
        assert_eq!(
            crate::light_headers::project_headers(&conn, 1)
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            [&7]
        );
    }

    /// A pass that reads no header still forgets the headers of lights the
    /// catalog no longer holds.
    #[test]
    fn a_pass_forgets_removed_lights_even_when_it_reads_nothing() {
        let temp = tempfile::tempdir().unwrap();
        let catalog = temp.path().join("catalog.sqlite");
        let conn = Connection::open(&catalog).unwrap();
        crate::ts_schema::apply_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata, guid)
             VALUES (7, 1, 1, 0, 'Ha', 0, '{}', 'seven'), (8, 1, 1, 0, 'Ha', 0, '{}', 'eight')",
            [],
        )
        .unwrap();
        let light = temp.path().join("light.fits");
        std::fs::write(&light, b"bytes").unwrap();
        let header = KeptHeader::new(
            &light,
            crate::light_headers::FileStamp::of(&light).unwrap(),
            crate::commands::import::headers::FrameMeta::default(),
        );
        crate::light_headers::store(&conn, [(7, &header), (8, &header)]).unwrap();
        conn.execute("DELETE FROM acquiredimage WHERE Id = 8", [])
            .unwrap();
        let path = catalog.to_string_lossy().into_owned();
        let reader = read_only(&path).unwrap();
        assert!(crate::light_headers::has_orphans(&reader));
        tidy_headers(&reader, &path);
        assert!(!crate::light_headers::has_orphans(&reader));
        let kept: Vec<i32> = conn
            .prepare("SELECT image_id FROM psf_guard_light_header")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(kept, [7]);
    }

    #[test]
    fn views_hold_back_while_grading_runs_and_after_asking_or_failing() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut views = ViewAsks::default();
        assert!(views.may_ask(false, start));
        assert!(!views.may_ask(true, start), "grading asks when it ends");

        // After a request, the views wait their interval.
        views.requested_at = Some(start);
        assert!(!views.may_ask(false, at(60)));
        assert!(views.may_ask(false, at(VIEW_ASK_INTERVAL.as_secs())));

        // A pass that failed holds them back longer.
        views.failed = Some(at(10));
        assert!(!views.may_ask(false, at(VIEW_ASK_INTERVAL.as_secs() + 10)));
        assert!(views.may_ask(false, at(VIEW_ASK_AFTER_FAILURE.as_secs() + 10)));
    }

    #[test]
    fn a_request_during_a_pass_runs_one_more() {
        let state = AppState::new_for_test(Connection::open_in_memory().unwrap());
        let mut queue = Queue {
            pass_requested: true,
            ..Queue::default()
        };
        let next = |queue: &mut Queue| CalibrationHealth::next(&state, "c", queue, Instant::now());
        assert_eq!(next(&mut queue), Step::Run(()));
        queue.pass_requested = true;
        assert_eq!(next(&mut queue), Step::Run(()), "one more pass");
        assert_eq!(next(&mut queue), Step::Exit, "then it stops");
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
