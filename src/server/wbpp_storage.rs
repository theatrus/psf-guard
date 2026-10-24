//! Lifetime and volume-pressure cleanup for PSF Guard's WBPP run folders.
//! Only marked runs or legacy runs with PSF Guard's runner signature are managed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::job_line::catalog::{CatalogJob, CatalogWork, Step};
use super::state::AppState;
use crate::concurrency::Stop;

const MARKER: &str = ".psf-guard-wbpp.json";
const JOB: &str = "wbpp-storage";
const NAP: Duration = Duration::from_millis(500);
type UsageProbe<'a> = dyn Fn(&Path, u8) -> Option<(f64, u64)> + 'a;

#[derive(Default)]
pub struct Manager {
    revision: AtomicU64,
    active: Arc<Mutex<HashMap<PathBuf, usize>>>,
    index_write: Mutex<()>,
    report: Mutex<Report>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Report {
    pub checked_at: Option<i64>,
    pub runs: usize,
    pub intermediate_bytes: u64,
    pub master_bytes: u64,
    pub deleted_files: usize,
    pub freed_bytes: u64,
    pub volumes: Vec<Volume>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Volume {
    pub path: PathBuf,
    pub used_percent: f64,
    pub max_percent: u8,
    pub over_limit: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Status {
    pub running: bool,
    pub current: Option<String>,
    pub waiting_for: Option<String>,
}

/// Holding this guard excludes a run from cleanup, including downloads and copies.
pub struct Protected {
    active: Arc<Mutex<HashMap<PathBuf, usize>>>,
    path: PathBuf,
}
impl Drop for Protected {
    fn drop(&mut self) {
        let mut active = self.active.lock().unwrap();
        if let Some(count) = active.get_mut(&self.path) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.path);
            }
        }
    }
}

fn resolved(path: &Path) -> PathBuf {
    if let Ok(path) = dunce::canonicalize(path) {
        return path;
    }
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                result.pop();
            }
            other => {
                result.push(other);
                // Resolve each existing prefix before interpreting a later '..'.
                // This also makes /var and /private/var identical on macOS.
                if let Ok(canonical) = dunce::canonicalize(&result) {
                    result = canonical;
                }
            }
        }
    }
    result
}

impl Manager {
    pub fn protect(&self, path: &Path) -> Protected {
        let path = resolved(path);
        *self.active.lock().unwrap().entry(path.clone()).or_default() += 1;
        Protected {
            active: Arc::clone(&self.active),
            path,
        }
    }
    fn policy_changed(&self) {
        // Synchronize with a deletion already in progress. Once this returns,
        // a pass planned under an earlier policy cannot remove another file.
        let _active = self.active.lock().unwrap();
        self.revision.fetch_add(1, Ordering::AcqRel);
    }
    pub fn report(&self) -> Report {
        self.report.lock().unwrap().clone()
    }
}

#[derive(Serialize, Deserialize)]
struct RunRecord {
    version: u8,
    started_at: i64,
    finished_at: Option<i64>,
    #[serde(default)]
    pid: Option<u32>,
}

fn write_json(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("WBPP metadata has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(file.as_file_mut(), value)?;
    file.as_file_mut().flush()?;
    file.as_file().sync_all()?;
    file.persist(path)?;
    Ok(())
}

fn index_path(state: &AppState) -> Option<PathBuf> {
    state.registry_path.read().unwrap().as_ref().map(|path| {
        path.with_file_name(format!(
            "{}.wbpp-roots.json",
            path.file_name().unwrap_or_default().to_string_lossy()
        ))
    })
}

/// Record override roots as well, so changing a setting or restarting cannot orphan them.
pub fn remember_roots(state: &AppState) -> anyhow::Result<()> {
    let _one = state.wbpp_storage.index_write.lock().unwrap();
    let roots = roots(state)?;
    if let Some(index) = index_path(state) {
        write_json(&index, &roots)?;
    }
    Ok(())
}

pub fn started(state: &AppState, path: &Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(path)?;
    let _one = state.wbpp_storage.index_write.lock().unwrap();
    if let Some(index) = index_path(state) {
        let mut roots: Vec<PathBuf> = match std::fs::read(&index) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => return Err(error.into()),
        };
        let root = resolved(
            path.parent()
                .ok_or_else(|| anyhow::anyhow!("WBPP run has no parent"))?,
        );
        if !roots.contains(&root) {
            roots.push(root);
            write_json(&index, &roots)?;
        }
    }
    write_json(
        &path.join(MARKER),
        &RunRecord {
            version: 1,
            started_at: chrono::Utc::now().timestamp(),
            finished_at: None,
            pid: None,
        },
    )
}

pub fn record_pid(path: &Path, pid: u32) -> anyhow::Result<()> {
    let file = path.join(MARKER);
    let mut record: RunRecord = serde_json::from_slice(&std::fs::read(&file)?)?;
    record.pid = Some(pid);
    write_json(&file, &record)
}

pub fn finished(path: &Path) -> anyhow::Result<()> {
    let file = path.join(MARKER);
    let mut record: RunRecord = serde_json::from_slice(&std::fs::read(&file)?)?;
    record.finished_at = Some(chrono::Utc::now().timestamp());
    record.pid = None;
    write_json(&file, &record)
}

// A server crash may leave PixInsight running. Preserve a run until that process exits.
#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else {
        return true;
    };
    if pid <= 0 {
        return true;
    }
    // SAFETY: signal zero only probes whether the process can be signalled.
    unsafe {
        libc::kill(pid, 0) == 0
            || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
    }
}
#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    use windows_sys::Win32::{
        Foundation::{CloseHandle, STILL_ACTIVE},
        System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION},
    };
    // SAFETY: query-only handle, closed before returning; an inaccessible process is protected.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            return windows_sys::Win32::Foundation::GetLastError() != 87;
        }
        let mut code = 0;
        let alive = GetExitCodeProcess(handle, &mut code) == 0 || code == STILL_ACTIVE as u32;
        CloseHandle(handle);
        alive
    }
}
#[cfg(not(any(unix, windows)))]
fn process_alive(_: u32) -> bool {
    true
}

#[derive(Clone)]
struct File {
    path: PathBuf,
    bytes: u64,
    master: bool,
    age_from: i64,
}
struct Run {
    active: bool,
    path: PathBuf,
    finished: i64,
    files: Vec<File>,
}

fn unix_modified(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| age.as_secs() as i64)
        .unwrap_or(i64::MAX)
}

fn walk(
    path: &Path,
    master: bool,
    files: &mut Vec<File>,
    keep_going: &dyn Fn() -> bool,
) -> std::io::Result<()> {
    for entry in std::fs::read_dir(path)? {
        if !keep_going() {
            return Err(std::io::ErrorKind::Interrupted.into());
        }
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if entry.file_name() == MARKER {
            continue;
        }
        let master = master || entry.file_name() == "master";
        if kind.is_dir() {
            walk(&entry.path(), master, files, keep_going)?;
        } else if kind.is_file() {
            let meta = entry.metadata()?;
            files.push(File {
                path: entry.path(),
                bytes: meta.len(),
                master,
                age_from: unix_modified(&meta),
            });
        }
    }
    Ok(())
}

fn legacy_record(path: &Path) -> Option<RunRecord> {
    let name = path.file_name()?.to_str()?;
    // Run folders end in YYYYMMDD-HHMMSS. An exported tree is not a managed run.
    let stamp = name.get(name.len().checked_sub(15)?..)?;
    chrono::NaiveDateTime::parse_from_str(stamp, "%Y%m%d-%H%M%S").ok()?;
    if !path.join("pixinsight.log").is_file() {
        return None;
    }
    let runner = path.join("run-wbpp.js");
    if std::fs::symlink_metadata(&runner)
        .ok()?
        .file_type()
        .is_symlink()
    {
        return None;
    }
    let header = std::fs::read_to_string(runner).ok()?;
    if !header
        .lines()
        .take(12)
        .any(|line| line.contains("Generated by PSF Guard for WBPP"))
    {
        return None;
    }
    let at = unix_modified(&std::fs::metadata(path).ok()?);
    Some(RunRecord {
        version: 1,
        started_at: at,
        finished_at: Some(at),
        pid: None,
    })
}

fn read_run(
    path: &Path,
    now: i64,
    active: bool,
    keep_going: &dyn Fn() -> bool,
) -> anyhow::Result<Option<Run>> {
    if !std::fs::symlink_metadata(path)?.file_type().is_dir() {
        return Ok(None);
    }
    let marker = path.join(MARKER);
    if std::fs::symlink_metadata(&marker).is_ok_and(|m| m.file_type().is_symlink()) {
        return Ok(None);
    }
    let mut record = match std::fs::read(&marker) {
        Ok(bytes) => serde_json::from_slice::<RunRecord>(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => match legacy_record(path) {
            Some(record) => record,
            None => return Ok(None),
        },
        Err(error) => return Err(error.into()),
    };
    if record.version != 1 {
        return Ok(None);
    }
    let active = active || record.pid.is_some_and(process_alive);
    let mut files = Vec::new();
    walk(path, false, &mut files, keep_going)?;
    let newest = files
        .iter()
        .map(|file| file.age_from)
        .max()
        .unwrap_or(record.started_at);
    // Interrupted runs without a live child are dated by their last write.
    let finished = if active {
        now
    } else {
        record.finished_at.unwrap_or(newest.max(record.started_at))
    };
    if finished > now {
        return Ok(None);
    }
    if !active && !marker.exists() {
        record.finished_at = Some(finished);
        record.pid = None;
        write_json(&marker, &record)?;
    }
    for file in &mut files {
        file.age_from = file.age_from.max(finished);
    }
    files.sort_by_key(|file| file.age_from);
    Ok(Some(Run {
        active,
        path: resolved(path),
        finished,
        files,
    }))
}

/// Files older than this lifetime expire. Zero disables age-based deletion.
fn expired(at: i64, now: i64, days: u32) -> bool {
    days > 0 && now.saturating_sub(at) >= i64::from(days) * 86400
}

fn roots(state: &AppState) -> anyhow::Result<Vec<PathBuf>> {
    let settings =
        super::wbpp_run::configured(state).map_err(|error| anyhow::anyhow!("{error:?}"))?;
    let mut roots = HashSet::new();
    for ctx in state.all_databases() {
        roots.insert(resolved(&super::wbpp_run::run_root(
            None,
            settings.runs_dir.as_deref(),
            ctx.export_dir.as_deref(),
            &ctx.stack_root,
            &ctx.id,
        )));
        // Include the former location after a setting or storage-root change.
        roots.insert(resolved(&ctx.stack_root.join(super::storage::WBPP_RUNS)));
        if let Some(export) = &ctx.export_dir {
            roots.insert(resolved(&export.join("wbpp")));
        }
    }
    if let Some(index) = index_path(state) {
        match std::fs::read(index) {
            Ok(bytes) => roots.extend(serde_json::from_slice::<Vec<PathBuf>>(&bytes)?),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    let mut roots: Vec<_> = roots.into_iter().collect();
    roots.sort();
    Ok(roots)
}

fn candidates(
    state: &AppState,
    now: i64,
    report: &mut Report,
    job: &CatalogJob<Cleanup>,
    stop: &Stop,
) -> anyhow::Result<Vec<Run>> {
    let mut runs = Vec::new();
    let mut seen = HashSet::new();
    for root in roots(state)? {
        let entries = match std::fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                report.errors.push(format!("{}: {error}", root.display()));
                continue;
            }
        };
        for entry in entries {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    report.errors.push(error.to_string());
                    continue;
                }
            };
            let path = resolved(&entry.path());
            if !seen.insert(path.clone()) || entry.file_type()?.is_symlink() {
                continue;
            }
            let active = state
                .wbpp_storage
                .active
                .lock()
                .unwrap()
                .contains_key(&path);
            let result = read_run(&path, now, active, &|| {
                if stop.requested() || state.shutdown.is_stopping() {
                    return false;
                }
                state.wait_for_interactive_work(NAP, |wait| {
                    job.update(|s| s.waiting_for = wait.map(str::to_string))
                });
                !stop.requested() && !state.shutdown.is_stopping()
            });
            match result {
                Ok(Some(run)) => runs.push(run),
                Ok(None) => {}
                Err(error) => report.errors.push(format!("{}: {error}", path.display())),
            }
        }
    }
    runs.sort_by_key(|run| run.finished);
    Ok(runs)
}

/// Remove files one at a time; never follow a replaced directory or a symlink.
fn remove_file(manager: &Manager, revision: u64, run: &Path, file: &File) -> std::io::Result<bool> {
    let active = manager.active.lock().unwrap();
    if manager.revision.load(Ordering::Acquire) != revision || active.contains_key(run) {
        return Ok(false);
    }
    let relative = file.path.strip_prefix(run).map_err(std::io::Error::other)?;
    let mut probe = run.to_path_buf();
    if std::fs::symlink_metadata(&probe)?.file_type().is_symlink() {
        return Ok(false);
    }
    for part in relative.components() {
        probe.push(part);
        if std::fs::symlink_metadata(&probe)?.file_type().is_symlink() {
            return Ok(false);
        }
    }
    let meta = std::fs::symlink_metadata(&file.path)?;
    if !meta.is_file() || meta.len() != file.bytes || unix_modified(&meta) > file.age_from {
        return Ok(false);
    }
    std::fs::remove_file(&file.path)?;
    Ok(true)
}

pub(crate) struct Cleanup;
impl CatalogWork for Cleanup {
    type Queue = bool;
    type Status = Status;
    type Item = ();
    const RUNNER: &'static str = "wbpp-cleanup";
    const SHUTDOWN_WAITS_FOR: Option<&'static str> = Some("WBPP cleanup");
    fn next(_: &AppState, _: &str, queue: &mut bool, _: Instant) -> Step<()> {
        if std::mem::take(queue) {
            Step::Run(())
        } else {
            Step::Exit
        }
    }
    fn has_work(queue: &bool) -> bool {
        *queue
    }
    fn run(state: &Arc<AppState>, _: &str, job: &CatalogJob<Self>, _: (), stop: &Stop) {
        let result = cleanup(state, job, stop);
        if let Err(error) = result {
            state.wbpp_storage.report.lock().unwrap().errors = vec![error.to_string()];
        }
        job.update(|status| *status = Status::default());
    }
    fn runner_failed(_: &mut bool, status: &mut Status) {
        *status = Status::default();
    }
}

fn cleanup(state: &Arc<AppState>, job: &CatalogJob<Cleanup>, stop: &Stop) -> anyhow::Result<()> {
    cleanup_with_usage(state, job, stop, &super::cache_budget::usage_for_path)
}

fn effective_limit(state: &AppState, path: &Path, configured: u8) -> u8 {
    if configured == 100 {
        return 100;
    }
    super::cache_budget::limit_on_path(&state.cache_budget, path)
        .map_or(configured, |limit| limit.min(configured))
}

fn cleanup_with_usage(
    state: &Arc<AppState>,
    job: &CatalogJob<Cleanup>,
    stop: &Stop,
    usage_for_path: &UsageProbe<'_>,
) -> anyhow::Result<()> {
    let revision = state.wbpp_storage.revision.load(Ordering::Acquire);
    let policy = super::wbpp_run::configured(state)
        .map_err(|error| anyhow::anyhow!("{error:?}"))?
        .storage;
    policy.validate().map_err(anyhow::Error::msg)?;
    let now = chrono::Utc::now().timestamp();
    let mut report = Report {
        checked_at: Some(now),
        ..Default::default()
    };
    job.update(|s| {
        s.running = true;
        s.current = Some("Measuring WBPP folders".into());
    });
    let runs = candidates(state, now, &mut report, job, stop)?;
    report.runs = runs.len();
    let mut removed = HashSet::new();
    let mut pressured = HashSet::new();
    // Expire intermediates, then reclaim more intermediate space under pressure,
    // and only then expire masters. Masters retained forever are never candidates.
    for tier in 0..3 {
        for run in &runs {
            if run.active {
                continue;
            }
            let limit = effective_limit(state, &run.path, policy.max_volume_percent);
            let key = super::cache_budget::device_of(&run.path)
                .map(|device| format!("device:{device}"))
                .unwrap_or_else(|| run.path.parent().unwrap_or(&run.path).display().to_string());
            let usage = usage_for_path(&run.path, limit);
            let pressure = usage.is_some_and(|usage| limit < 100 && usage.0 > f64::from(limit));
            if pressure {
                pressured.insert(key.clone());
            }
            let mut need = if pressured.contains(&key) {
                usage.map_or(0, |u| u.1)
            } else {
                0
            };
            for file in &run.files {
                if stop.requested() || state.shutdown.is_stopping() {
                    return Ok(());
                }
                state.wait_for_interactive_work(NAP, |wait| {
                    job.update(|s| s.waiting_for = wait.map(str::to_string))
                });
                let eligible = match tier {
                    0 => !file.master && expired(file.age_from, now, policy.intermediate_days),
                    1 => !file.master && need > 0,
                    _ => file.master && expired(file.age_from, now, policy.master_days),
                };
                if !eligible || removed.contains(&file.path) {
                    continue;
                }
                job.update(|s| s.current = Some(format!("Removing {}", file.path.display())));
                match remove_file(&state.wbpp_storage, revision, &run.path, file) {
                    Ok(true) => {
                        report.deleted_files += 1;
                        report.freed_bytes += file.bytes;
                        need = need.saturating_sub(file.bytes);
                        removed.insert(file.path.clone());
                    }
                    Ok(false) => {}
                    Err(error) => report
                        .errors
                        .push(format!("{}: {error}", file.path.display())),
                }
            }
        }
    }
    let mut volumes = HashSet::new();
    for run in &runs {
        for file in &run.files {
            if removed.contains(&file.path) {
                continue;
            }
            if file.master {
                report.master_bytes += file.bytes;
            } else {
                report.intermediate_bytes += file.bytes;
            }
        }
        let limit = effective_limit(state, &run.path, policy.max_volume_percent);
        if let Some((used, _)) = usage_for_path(&run.path, limit) {
            // One report per root; several roots on one disk share the same percentage.
            let root = run.path.parent().unwrap().to_path_buf();
            if volumes.insert(root.clone()) {
                report.volumes.push(Volume {
                    path: root,
                    used_percent: used,
                    max_percent: limit,
                    over_limit: limit < 100 && used > f64::from(limit),
                });
            }
        }
        // Remove empty directories, without traversing links. A retained master
        // or an error leaves the run and its ownership marker in place.
        if !run.active {
            prune_empty(&state.wbpp_storage, &run.path);
        }
    }
    refresh_run_entries(state, &removed);
    report.runs = runs.iter().filter(|run| run.path.is_dir()).count();
    *state.wbpp_storage.report.lock().unwrap() = report;
    Ok(())
}

fn refresh_run_entries(state: &AppState, removed: &HashSet<PathBuf>) {
    for ctx in state.all_databases() {
        let snapshot = ctx.wbpp_run.read().unwrap().progress.clone();
        if snapshot.running || snapshot.work_dir.is_empty() {
            continue;
        }
        let work = resolved(Path::new(&snapshot.work_dir));
        let missing: HashSet<_> = snapshot
            .outputs
            .iter()
            .filter(|file| removed.contains(&work.join("wbpp-out").join(&file.path)))
            .map(|file| file.path.clone())
            .collect();
        let gone = !work.is_dir();
        let mut current = ctx.wbpp_run.write().unwrap();
        if current.progress.work_dir != snapshot.work_dir || current.progress.running {
            continue;
        }
        if gone {
            current.progress = super::wbpp_run::WbppRunProgress::default();
        } else {
            current
                .progress
                .outputs
                .retain(|file| !missing.contains(&file.path));
        }
    }
}

fn prune_empty(manager: &Manager, path: &Path) {
    let active = manager.active.lock().unwrap();
    if active.contains_key(path) {
        return;
    }
    fn prune(path: &Path) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                prune(&entry.path());
                let _ = std::fs::remove_dir(entry.path());
            }
        }
    }
    prune(path);
    if std::fs::read_dir(path)
        .is_ok_and(|entries| entries.flatten().all(|entry| entry.file_name() == MARKER))
    {
        let _ = std::fs::remove_file(path.join(MARKER));
        let _ = std::fs::remove_dir(path);
    }
}

pub fn policy_changed(state: &Arc<AppState>) {
    state.wbpp_storage.policy_changed();
    let job = state.catalog_jobs.wbpp_storage.job(JOB);
    job.stop(|_| {});
    request(state);
}

pub fn request(state: &Arc<AppState>) {
    state
        .catalog_jobs
        .wbpp_storage
        .request(state, JOB, |pending| *pending = true);
}
pub fn status(state: &AppState) -> Status {
    state
        .catalog_jobs
        .wbpp_storage
        .existing(JOB)
        .map(|job| job.status())
        .unwrap_or_default()
}
pub fn spawn(state: &Arc<AppState>) {
    super::schedule::every(
        state,
        "WBPP storage",
        super::schedule::Every::after(Duration::from_secs(60), Duration::from_secs(300)),
        |state| async move {
            request(&state);
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_registry::{DbRegistry, PixInsightSettings, WbppStoragePolicy};

    fn fixture(policy: WbppStoragePolicy) -> (tempfile::TempDir, Arc<AppState>) {
        let dir = tempfile::tempdir().unwrap();
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        state.databases.write().unwrap().clear();
        let registry = dir.path().join("registry.json");
        DbRegistry {
            pixinsight: Some(PixInsightSettings {
                storage: policy,
                ..Default::default()
            }),
            ..Default::default()
        }
        .save(&registry)
        .unwrap();
        state.set_registry_path(Some(registry));
        (dir, state)
    }
    fn make_run(state: &AppState, path: &Path, days: i64) {
        started(state, path).unwrap();
        let at = chrono::Utc::now().timestamp() - days * 86400;
        write_json(
            &path.join(MARKER),
            &RunRecord {
                version: 1,
                started_at: at,
                finished_at: Some(at),
                pid: None,
            },
        )
        .unwrap();
    }
    fn file(path: &Path, days: i64) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, [42; 100]).unwrap();
        let at = chrono::Utc::now().timestamp() - days * 86400;
        filetime::set_file_mtime(path, filetime::FileTime::from_unix_time(at, 0)).unwrap();
    }
    fn pass(state: &Arc<AppState>, usage: &UsageProbe<'_>) {
        cleanup_with_usage(
            state,
            &CatalogJob::<Cleanup>::default(),
            &Stop::new(),
            usage,
        )
        .unwrap();
    }
    #[test]
    fn lifetimes_remove_intermediates_before_masters_and_eventually_remove_the_run() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("runs/target");
        make_run(&state, &run, 10);
        let light = run.join("wbpp-out/registered/light.xisf");
        file(&light, 10);
        let master = run.join("wbpp-out/master/masterLight.xisf");
        file(&master, 10);
        pass(&state, &|_, _| Some((40.0, 0)));
        assert!(!light.exists());
        assert!(master.exists());
        assert!(run.join(MARKER).exists());
        assert_eq!(state.wbpp_storage.report().freed_bytes, 100);
        write_json(
            &run.join(MARKER),
            &RunRecord {
                version: 1,
                started_at: 0,
                finished_at: Some(0),
                pid: None,
            },
        )
        .unwrap();
        file(&master, 100);
        pass(&state, &|_, _| Some((40.0, 0)));
        assert!(!run.exists(), "empty run and ownership entry are removed");
    }
    #[test]
    fn pressure_deletes_young_intermediates_but_preserves_forever_masters_and_sources() {
        let (dir, state) = fixture(WbppStoragePolicy {
            master_days: 0,
            ..Default::default()
        });
        let run = dir.path().join("runs/target");
        make_run(&state, &run, 2);
        let light = run.join("wbpp-out/calibrated/light.xisf");
        file(&light, 2);
        let master = run.join("wbpp-out/master/masterLight.xisf");
        file(&master, 200);
        let source = dir.path().join("raw/source.fits");
        file(&source, 200);
        pass(&state, &|_, limit| {
            assert_eq!(limit, 90);
            Some((95.0, 1000))
        });
        assert!(!light.exists());
        assert!(master.exists());
        assert!(source.exists());
        assert!(state.wbpp_storage.report().volumes[0].over_limit);
    }
    #[test]
    fn pressure_removes_oldest_run_first_and_stops_after_reclaiming_space() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let older = dir.path().join("runs/older");
        make_run(&state, &older, 3);
        let newer = dir.path().join("runs/newer");
        make_run(&state, &newer, 1);
        let old = older.join("wbpp-out/registered/old.xisf");
        file(&old, 3);
        let new = newer.join("wbpp-out/registered/new.xisf");
        file(&new, 1);
        pass(&state, &|_, _| {
            if old.exists() {
                Some((92.0, 100))
            } else {
                Some((88.0, 0))
            }
        });
        assert!(!old.exists());
        assert!(new.exists());
    }
    #[test]
    fn active_runs_and_copies_are_counted_but_never_deleted() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("runs/target");
        make_run(&state, &run, 100);
        let master = run.join("wbpp-out/master/masterLight.xisf");
        file(&master, 100);
        let guard = state.wbpp_storage.protect(&run);
        let second = state.wbpp_storage.protect(&run);
        drop(guard);
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(master.exists());
        assert_eq!(state.wbpp_storage.report().master_bytes, 100);
        drop(second);
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(!run.exists());
    }
    #[test]
    fn override_roots_survive_restart_and_setting_changes() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("custom/db/old");
        make_run(&state, &run, 100);
        let master = run.join("wbpp-out/master/masterLight.xisf");
        file(&master, 100);
        let restarted = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        restarted.databases.write().unwrap().clear();
        restarted.set_registry_path(state.registry_path.read().unwrap().clone());
        pass(&restarted, &|_, _| Some((10.0, 0)));
        assert!(!run.exists());
    }
    #[test]
    fn recently_written_masters_and_unknown_directories_are_preserved() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("runs/target");
        make_run(&state, &run, 100);
        let master = run.join("wbpp-out/master/recent.xisf");
        file(&master, 1);
        let unrelated = dir.path().join("runs/user-work/master.xisf");
        file(&unrelated, 100);
        pass(&state, &|_, _| Some((10.0, 0)));
        assert!(master.exists());
        assert!(unrelated.exists());
    }
    #[cfg(unix)]
    #[test]
    fn cleanup_does_not_follow_symlinks_or_delete_a_live_orphan_process() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("runs/target");
        make_run(&state, &run, 100);
        let outside = dir.path().join("originals");
        let source = outside.join("frame.fits");
        file(&source, 100);
        std::os::unix::fs::symlink(&outside, run.join("linked")).unwrap();
        let own = run.join("wbpp-out/registered/frame.xisf");
        file(&own, 100);
        record_pid(&run, std::process::id()).unwrap();
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(own.exists());
        assert!(source.exists());
        finished(&run).unwrap();
        // Finished now means the file's retention begins now.
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(!own.exists());
        assert!(source.exists());
    }
    #[test]
    fn legacy_runs_require_the_timestamp_and_generated_runner_signature() {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("project_M45-20260101-010203");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(
            run.join("run-wbpp.js"),
            "// Generated by PSF Guard for WBPP 3.1.0.\n",
        )
        .unwrap();
        file(&run.join("pixinsight.log"), 100);
        file(&run.join("wbpp-out/master/M45.xisf"), 100);
        assert!(
            read_run(&run, chrono::Utc::now().timestamp(), false, &|| true)
                .unwrap()
                .is_some()
        );
        assert!(run.join(MARKER).is_file());
        let unknown = dir.path().join("my-export");
        std::fs::create_dir_all(&unknown).unwrap();
        std::fs::write(
            unknown.join("run-wbpp.js"),
            "// Generated by PSF Guard for WBPP 3.1.0.\n",
        )
        .unwrap();
        assert!(
            read_run(&unknown, chrono::Utc::now().timestamp(), false, &|| true)
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn hundred_percent_disables_pressure_cleanup_but_not_expiry() {
        let (dir, state) = fixture(WbppStoragePolicy {
            max_volume_percent: 100,
            ..Default::default()
        });
        let run = dir.path().join("runs/young");
        make_run(&state, &run, 2);
        let young = run.join("wbpp-out/registered/young.xisf");
        file(&young, 2);
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(young.exists());
        assert!(!state.wbpp_storage.report().volumes[0].over_limit);
        let old = dir.path().join("runs/old");
        make_run(&state, &old, 100);
        let expired = old.join("wbpp-out/master/old.xisf");
        file(&expired, 100);
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(!old.exists());
        assert!(young.exists());
    }

    #[test]
    fn protection_matches_a_path_with_nonexistent_parent_components() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let alias = dir.path().join("not-created-yet/../runs/target");
        let guard = state.wbpp_storage.protect(&alias);
        make_run(&state, &alias, 100);
        let master = dir.path().join("runs/target/wbpp-out/master/master.xisf");
        file(&master, 100);
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(master.exists());
        drop(guard);
        pass(&state, &|_, _| Some((99.0, 10000)));
        assert!(!master.exists());
    }

    #[cfg(unix)]
    #[test]
    fn cleanup_does_not_need_to_write_new_metadata_before_freeing_space() {
        use std::os::unix::fs::PermissionsExt;
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("runs/old");
        make_run(&state, &run, 100);
        let old = run.join("wbpp-out/registered/frame.xisf");
        file(&old, 100);
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        let result = cleanup_with_usage(
            &state,
            &CatalogJob::<Cleanup>::default(),
            &Stop::new(),
            &|_, _| Some((99.0, 10000)),
        );
        std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        result.unwrap();
        assert!(!run.exists());
    }

    #[test]
    fn a_saved_policy_change_invalidates_an_already_planned_cleanup() {
        let (dir, state) = fixture(WbppStoragePolicy::default());
        let run = dir.path().join("runs/old");
        make_run(&state, &run, 100);
        let master = run.join("wbpp-out/master/master.xisf");
        file(&master, 100);
        let changed = std::sync::atomic::AtomicBool::new(false);
        pass(&state, &|_, _| {
            if !changed.swap(true, Ordering::SeqCst) {
                state.wbpp_storage.policy_changed();
            }
            Some((99.0, 10000))
        });
        assert!(
            master.exists(),
            "an old pass cannot delete after a new policy was saved"
        );
    }

    #[test]
    fn invalid_policies_are_rejected_and_old_settings_keep_defaults() {
        let old: PixInsightSettings = serde_json::from_str(r#"{"runs_dir":"/data/wbpp"}"#).unwrap();
        assert_eq!(old.storage, WbppStoragePolicy::default());
        assert!(WbppStoragePolicy {
            max_volume_percent: 101,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(WbppStoragePolicy {
            master_days: 1,
            ..Default::default()
        }
        .validate()
        .is_err());
        assert!(WbppStoragePolicy {
            master_days: 0,
            ..Default::default()
        }
        .validate()
        .is_ok());
    }
}
