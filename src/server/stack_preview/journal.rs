//! The job journal: what was queued or running, kept on disk so a restart
//! picks it up again.
//!
//! Stack builds (mono and color) in the order they would run, automatic
//! refreshes still settling, and the WBPP line with any run a restart cut
//! off. A writer compares a fresh snapshot with the last one it wrote every
//! few seconds and replaces the file atomically when they differ, and the
//! server writes it once more on a clean shutdown. The file sits beside the
//! database registry, so a server without one keeps no journal.

use super::automatic::RefreshReason;
use super::color::StackColorRequest;
use super::{StackPreviewJob, StackPreviewRequest};
use crate::server::state::AppState;
use crate::server::wbpp_run::StartWbppRunRequest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const SCHEMA_VERSION: u32 = 1;
/// How often the writer looks for changes.
const WRITE_INTERVAL: Duration = Duration::from_secs(3);
/// A build cut off this many times by restarts is not started again: it is
/// the likelier cause of the restarts.
const MAX_ATTEMPTS: u32 = 2;
/// How long an entry that could not be prepared after a restart (its disk
/// not mounted yet, say) keeps being retried.
const RETRY_FOR: Duration = Duration::from_secs(30 * 60);
const RETRY_EVERY: Duration = Duration::from_secs(60);

/// A stack build as it was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournaledStackJob {
    Mono {
        database_id: String,
        project_id: i32,
        automatic: bool,
        request: StackPreviewRequest,
        /// Restarts that have cut it off while it ran.
        #[serde(default)]
        attempts: u32,
        /// It was running when the journal was written.
        #[serde(default)]
        running: bool,
    },
    Color {
        database_id: String,
        project_id: i32,
        automatic: bool,
        request: StackColorRequest,
        #[serde(default)]
        attempts: u32,
        #[serde(default)]
        running: bool,
    },
}

impl PartialEq for StackPreviewRequest {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(other).ok()
    }
}

impl PartialEq for StackColorRequest {
    fn eq(&self, other: &Self) -> bool {
        serde_json::to_value(self).ok() == serde_json::to_value(other).ok()
    }
}

impl JournaledStackJob {
    /// A mono build, with the method it was given pinned so a restart
    /// rebuilds the same job even if the server's method has changed.
    pub fn mono(
        database_id: &str,
        project_id: i32,
        request: &StackPreviewRequest,
        job: &StackPreviewJob,
        automatic: bool,
    ) -> Self {
        let mut request = request.clone();
        request.method = Some(job.method.unwrap_or_default());
        Self::Mono {
            database_id: database_id.to_string(),
            project_id,
            automatic,
            request,
            attempts: 0,
            running: false,
        }
    }

    pub fn color(
        database_id: &str,
        project_id: i32,
        request: &StackColorRequest,
        automatic: bool,
    ) -> Self {
        Self::Color {
            database_id: database_id.to_string(),
            project_id,
            automatic,
            request: request.clone(),
            attempts: 0,
            running: false,
        }
    }

    pub fn set_automatic(&mut self, value: bool) {
        match self {
            Self::Mono { automatic, .. } | Self::Color { automatic, .. } => *automatic = value,
        }
    }

    pub(super) fn set_running(&mut self, value: bool) {
        match self {
            Self::Mono { running, .. } | Self::Color { running, .. } => *running = value,
        }
    }

    fn automatic(&self) -> bool {
        match self {
            Self::Mono { automatic, .. } | Self::Color { automatic, .. } => *automatic,
        }
    }

    /// Count a restart against a build that was running, before it is put
    /// back. `None` when it has been cut off too often to try again.
    fn after_restart(mut self) -> Option<Self> {
        match &mut self {
            Self::Mono {
                attempts, running, ..
            }
            | Self::Color {
                attempts, running, ..
            } => {
                if *running {
                    *attempts += 1;
                    *running = false;
                }
                if *attempts > MAX_ATTEMPTS {
                    return None;
                }
            }
        }
        Some(self)
    }
}

/// An automatic refresh still settling, due at a wall-clock time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JournaledRefresh {
    pub database_id: String,
    pub project_id: Option<i32>,
    pub reason: RefreshReason,
    pub due_unix: i64,
}

/// A WBPP run waiting, or cut off while it ran.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournaledWbppRun {
    pub db_id: String,
    pub request: StartWbppRunRequest,
    /// It was running when the journal was written.
    #[serde(default)]
    pub cut_off: bool,
    /// Its PixInsight, which may outlive the server.
    #[serde(default)]
    pub pid: Option<u32>,
}

impl PartialEq for JournaledWbppRun {
    fn eq(&self, other: &Self) -> bool {
        self.db_id == other.db_id
            && self.cut_off == other.cut_off
            && self.pid == other.pid
            && serde_json::to_value(&self.request).ok() == serde_json::to_value(&other.request).ok()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: u32,
    /// Running first, then the line, then entries still waiting to be
    /// restored.
    #[serde(default)]
    pub stack: Vec<JournaledStackJob>,
    #[serde(default)]
    pub automatic: Vec<JournaledRefresh>,
    /// A run cut off first, then the line.
    #[serde(default)]
    pub wbpp: Vec<JournaledWbppRun>,
}

impl Journal {
    fn is_empty(&self) -> bool {
        self.stack.is_empty() && self.automatic.is_empty() && self.wbpp.is_empty()
    }
}

/// Builds a restart could not prepare yet, retried for a while and kept in
/// the journal meanwhile.
static DEFERRED: Mutex<Vec<(JournaledStackJob, Instant)>> = Mutex::new(Vec::new());

/// The journal's file for a registry: `<registry stem>.jobs.json` beside it.
pub fn journal_path(registry_path: &Path) -> PathBuf {
    let stem = registry_path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "registry".into());
    registry_path.with_file_name(format!("{stem}.jobs.json"))
}

/// Everything queued or running now.
pub fn snapshot(state: &AppState) -> Journal {
    let mut wbpp = Vec::new();
    for ctx in state.all_databases() {
        if let Some((request, pid)) = ctx.wbpp_run.read().unwrap().running_request() {
            wbpp.push(JournaledWbppRun {
                db_id: ctx.id.clone(),
                request,
                cut_off: true,
                pid,
            });
        }
    }
    wbpp.extend(
        state
            .wbpp_queue
            .journal_entries()
            .into_iter()
            .map(|run| JournaledWbppRun {
                db_id: run.db_id,
                request: run.request,
                cut_off: false,
                pid: None,
            }),
    );
    let mut automatic = state.auto_stacks.journal_entries();
    automatic.sort_by(|left, right| {
        left.due_unix
            .cmp(&right.due_unix)
            .then_with(|| left.database_id.cmp(&right.database_id))
            .then_with(|| left.project_id.cmp(&right.project_id))
    });
    let mut stack = state.stack_previews.journal_entries();
    stack.extend(DEFERRED.lock().unwrap().iter().map(|(job, _)| job.clone()));
    Journal {
        schema_version: SCHEMA_VERSION,
        stack,
        automatic,
        wbpp,
    }
}

fn write(path: &Path, journal: &Journal) -> Result<(), String> {
    if journal.is_empty() {
        return match std::fs::remove_file(path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.to_string()),
            _ => Ok(()),
        };
    }
    super::stretch::write_json_atomic(path, journal)
}

fn entries<T: serde::de::DeserializeOwned>(value: &serde_json::Value, field: &str) -> Vec<T> {
    value
        .get(field)
        .and_then(|entries| entries.as_array())
        .into_iter()
        .flatten()
        .filter_map(|entry| match serde_json::from_value(entry.clone()) {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!(
                    "Dropping a job journal {field} entry that no longer reads: {error}"
                );
                None
            }
        })
        .collect()
}

/// Read entry by entry, so one that no longer parses after an upgrade costs
/// only itself.
fn read(path: &Path) -> Option<Journal> {
    let bytes = std::fs::read(path).ok()?;
    let value: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(value) => value,
        Err(error) => {
            tracing::warn!(
                "Ignoring an unreadable job journal at {}: {error}",
                path.display()
            );
            return None;
        }
    };
    let version = value
        .get("schema_version")
        .and_then(|version| version.as_u64());
    if version != Some(u64::from(SCHEMA_VERSION)) {
        tracing::warn!(
            "Ignoring a job journal of schema {version:?} at {}; this server reads {SCHEMA_VERSION}",
            path.display()
        );
        return None;
    }
    Some(Journal {
        schema_version: SCHEMA_VERSION,
        stack: entries(&value, "stack"),
        automatic: entries(&value, "automatic"),
        wbpp: entries(&value, "wbpp"),
    })
}

fn path_for(state: &AppState) -> Option<PathBuf> {
    state
        .registry_path
        .read()
        .unwrap()
        .as_deref()
        .map(journal_path)
}

/// Keep the journal current until the process ends.
pub fn spawn_writer(state: Arc<AppState>) {
    let Some(path) = path_for(&state) else {
        return;
    };
    tokio::spawn(async move {
        let mut last = read(&path).unwrap_or_default();
        loop {
            tokio::time::sleep(WRITE_INTERVAL).await;
            let current = snapshot(&state);
            if current == last {
                continue;
            }
            let write_path = path.clone();
            let journal = current.clone();
            match tokio::task::spawn_blocking(move || write(&write_path, &journal)).await {
                Ok(Ok(())) => last = current,
                Ok(Err(error)) => tracing::warn!("Could not save the job journal: {error}"),
                Err(error) => tracing::warn!("Job journal task failed: {error}"),
            }
        }
    });
}

/// Write the journal now, for a clean shutdown.
pub fn write_now(state: &AppState) {
    let Some(path) = path_for(state) else {
        return;
    };
    if let Err(error) = write(&path, &snapshot(state)) {
        tracing::warn!("Could not save the job journal at shutdown: {error}");
    }
}

/// Why one build was not put back.
enum NotRestored {
    /// For good: it finished, is already known, or its database is gone.
    Skip(String),
    /// For now: it could not be prepared, perhaps because its disk is not
    /// mounted yet.
    Retry(String),
}

/// Put back what the journal kept: stack builds first, in their order, then
/// settling refreshes, then the WBPP line. The scheduler waits until this is
/// done, so no refresh slips in ahead of a build that was waiting.
pub async fn restore(state: &Arc<AppState>) {
    let journal = path_for(state).and_then(|path| read(&path));
    if let Some(journal) = journal {
        tracing::info!(
            stack = journal.stack.len(),
            automatic = journal.automatic.len(),
            wbpp = journal.wbpp.len(),
            "Restoring the job queue from before the restart"
        );
        let mut restored_a_persons_build = false;
        for job in journal.stack {
            let Some(job) = job.after_restart() else {
                tracing::warn!(
                    "A stack build cut off by {MAX_ATTEMPTS} restarts in a row is not started again"
                );
                continue;
            };
            let automatic = job.automatic();
            match restore_stack_job(state, job.clone()).await {
                Ok(()) => restored_a_persons_build |= !automatic,
                Err(NotRestored::Skip(reason)) => {
                    tracing::info!("A queued stack build was not restored: {reason}")
                }
                Err(NotRestored::Retry(reason)) => {
                    tracing::info!("A queued stack build will be retried: {reason}");
                    DEFERRED.lock().unwrap().push((job, Instant::now()));
                }
            }
        }
        if restored_a_persons_build {
            super::interrupt_automatic(state);
        }
        for refresh in &journal.automatic {
            state.auto_stacks.restore(refresh);
        }
        let mut queued = Vec::new();
        for run in journal.wbpp {
            if run.cut_off {
                crate::server::wbpp_run::record_cut_off(state, &run.db_id, &run.request, run.pid);
            } else {
                queued.push((run.db_id, run.request));
            }
        }
        crate::server::wbpp_run::restore_queue(state, queued);
    }
    super::automatic::mark_ready();
    if !DEFERRED.lock().unwrap().is_empty() {
        let state = Arc::clone(state);
        tokio::spawn(async move { retry_deferred(state).await });
    }
}

async fn retry_deferred(state: Arc<AppState>) {
    loop {
        tokio::time::sleep(RETRY_EVERY).await;
        let pending = std::mem::take(&mut *DEFERRED.lock().unwrap());
        if pending.is_empty() {
            return;
        }
        for (job, since) in pending {
            match restore_stack_job(&state, job.clone()).await {
                Ok(()) => {}
                Err(NotRestored::Skip(reason)) => {
                    tracing::info!("A queued stack build was not restored: {reason}")
                }
                Err(NotRestored::Retry(reason)) if since.elapsed() >= RETRY_FOR => {
                    tracing::warn!("Gave up restoring a queued stack build: {reason}")
                }
                Err(NotRestored::Retry(_)) => DEFERRED.lock().unwrap().push((job, since)),
            }
        }
    }
}

async fn restore_stack_job(
    state: &Arc<AppState>,
    job: JournaledStackJob,
) -> Result<(), NotRestored> {
    let origin = job.clone();
    match job {
        JournaledStackJob::Mono {
            database_id,
            project_id,
            automatic,
            request,
            ..
        } => {
            let ctx = state
                .get_database(&database_id)
                .ok_or_else(|| NotRestored::Skip(format!("database {database_id} is gone")))?;
            let request_for_prepare = request.clone();
            let mut prepared = tokio::task::spawn_blocking(move || {
                super::prepare_job(&ctx, project_id, &request_for_prepare)
            })
            .await
            .map_err(|error| NotRestored::Retry(error.to_string()))?
            .map_err(|error| NotRestored::Retry(format!("{error:?}")))?;
            let job_id = prepared.public.job_id.clone();
            if state.stack_previews.get(&job_id).is_some() {
                return Err(NotRestored::Skip(format!("{job_id} is already known")));
            }
            // A forced rebuild shares the id of the build it replaces, whose
            // finished manifest stays until the new one lands.
            if !request.force
                && let Ok(bytes) =
                    std::fs::read(super::manifest_path(&prepared.stack_root, &job_id))
                && let Ok(existing) = serde_json::from_slice::<StackPreviewJob>(&bytes)
                && existing.state == super::StackJobState::Completed
            {
                return Err(NotRestored::Skip(format!(
                    "{job_id} finished before the restart"
                )));
            }
            prepared.public.automatic = automatic;
            if !state.stack_previews.insert(prepared.public.clone()) {
                return Err(NotRestored::Retry("too many stack jobs are active".into()));
            }
            super::enqueue_job(Arc::clone(state), prepared, origin);
            Ok(())
        }
        JournaledStackJob::Color {
            database_id,
            project_id,
            automatic,
            request,
            ..
        } => {
            let ctx = state
                .get_database(&database_id)
                .ok_or_else(|| NotRestored::Skip(format!("database {database_id} is gone")))?;
            let request_for_prepare = request.clone();
            let mut prepared = tokio::task::spawn_blocking(move || {
                super::color::prepare_color_job(&ctx, project_id, &request_for_prepare)
            })
            .await
            .map_err(|error| NotRestored::Retry(error.to_string()))?
            .map_err(|error| NotRestored::Retry(format!("{error:?}")))?;
            let job_id = prepared.public.job_id.clone();
            if state.stack_previews.get_color(&job_id).is_some() {
                return Err(NotRestored::Skip(format!("{job_id} is already known")));
            }
            let manifest = super::color::color_manifest_path(&prepared.stack_root, &job_id);
            if !request.force
                && let Ok(bytes) = std::fs::read(&manifest)
                && let Ok(existing) = serde_json::from_slice::<super::color::StackColorJob>(&bytes)
                && existing.state == super::StackJobState::Completed
                && super::color::color_job_artifacts_exist(&prepared.stack_root, &existing)
            {
                return Err(NotRestored::Skip(format!(
                    "{job_id} finished before the restart"
                )));
            }
            prepared.public.automatic = automatic;
            if !state.stack_previews.insert_color(prepared.public.clone()) {
                return Err(NotRestored::Retry("too many color jobs are active".into()));
            }
            super::color::enqueue_color_job(Arc::clone(state), prepared, origin);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mono(running: bool, attempts: u32) -> JournaledStackJob {
        JournaledStackJob::Mono {
            database_id: "db-a".into(),
            project_id: 7,
            automatic: true,
            request: serde_json::from_value(serde_json::json!({
                "image_ids": [1, 2, 3],
                "method": {"final_pass": "draft"}
            }))
            .unwrap(),
            attempts,
            running,
        }
    }

    #[test]
    fn the_journal_sits_beside_the_registry_and_round_trips() {
        assert_eq!(
            journal_path(Path::new("/etc/psf-guard/config.json")),
            PathBuf::from("/etc/psf-guard/config.jobs.json")
        );
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reg.jobs.json");
        let journal = Journal {
            schema_version: SCHEMA_VERSION,
            stack: vec![mono(true, 1)],
            automatic: vec![JournaledRefresh {
                database_id: "db-a".into(),
                project_id: None,
                reason: RefreshReason::Grade,
                due_unix: 1_800_000_000,
            }],
            wbpp: vec![JournaledWbppRun {
                db_id: "db-b".into(),
                request: StartWbppRunRequest {
                    project_id: Some(3),
                    ..Default::default()
                },
                cut_off: true,
                pid: Some(4242),
            }],
        };
        write(&path, &journal).unwrap();
        assert_eq!(read(&path), Some(journal));
        // An empty queue leaves no file behind.
        write(&path, &Journal::default()).unwrap();
        assert!(!path.exists());
        assert_eq!(read(&path), None);
    }

    #[test]
    fn one_entry_that_no_longer_reads_costs_only_itself() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reg.jobs.json");
        let good = serde_json::to_value(mono(false, 0)).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "stack": [{"kind": "mono", "database_id": "x"}, good],
                "automatic": [],
                "wbpp": [],
            }))
            .unwrap(),
        )
        .unwrap();
        let journal = read(&path).unwrap();
        assert_eq!(journal.stack, vec![mono(false, 0)]);
        // Another schema is set aside whole.
        std::fs::write(&path, br#"{"schema_version": 99, "stack": []}"#).unwrap();
        assert_eq!(read(&path), None);
    }

    #[test]
    fn a_build_cut_off_too_often_is_not_started_again() {
        // A waiting build never counts a restart.
        assert_eq!(mono(false, 0).after_restart(), Some(mono(false, 0)));
        assert_eq!(mono(true, 0).after_restart(), Some(mono(false, 1)));
        assert_eq!(mono(true, 1).after_restart(), Some(mono(false, 2)));
        assert_eq!(mono(true, MAX_ATTEMPTS).after_restart(), None);
        let mut adopted = mono(false, 0);
        adopted.set_automatic(false);
        assert!(!adopted.automatic());
    }
}
