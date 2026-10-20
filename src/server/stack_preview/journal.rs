//! The job journal: what was queued or running, kept on disk so a restart
//! picks it up again.
//!
//! Stack builds (mono and color) and the stretch, deconvolution and
//! RC-Astro processing runs that share their line, in the order they would
//! run; automatic refreshes still settling; and the WBPP line with any run a
//! restart cut off. A writer compares a fresh snapshot with the last one it wrote every
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

/// Version 2 added processing runs to `stack`; version 3 added `split` and
/// `job_id` to mono builds. An older journal has none and reads as it is.
const SCHEMA_VERSION: u32 = 3;
const OLDEST_SCHEMA_VERSION: u32 = 1;
/// How often the writer looks for changes.
const WRITE_INTERVAL: Duration = Duration::from_secs(3);
/// A build or processing run cut off this many times by restarts is not
/// started again: it is the likelier cause of the restarts.
const MAX_ATTEMPTS: u32 = 2;
/// How long an entry that could not be prepared after a restart (its disk
/// not mounted yet, say) keeps being retried.
const RETRY_FOR: Duration = Duration::from_secs(30 * 60);
pub(super) const RETRY_EVERY: Duration = Duration::from_secs(60);

/// What the journal's last write needs from a server stop: the builds and
/// runs the stop interrupted, in the order they would have run, and the
/// processing runs that published after the stop began.
#[derive(Default)]
pub(super) struct StopRecord {
    /// `None` until a stop begins. A stopped build is no longer tracked by
    /// the line; this keeps its journal record.
    interrupted: std::sync::Mutex<Option<Vec<(String, JournaledStackJob)>>>,
    completed: std::sync::Mutex<std::collections::HashSet<String>>,
}

impl StopRecord {
    /// A stop began, interrupting `interrupted`.
    pub(super) fn begin(&self, interrupted: Vec<(String, JournaledStackJob)>) {
        *self.interrupted.lock().unwrap() = Some(interrupted);
    }

    /// What the stop interrupted, once one began.
    pub(super) fn interrupted(&self) -> Option<Vec<(String, JournaledStackJob)>> {
        self.interrupted.lock().unwrap().clone()
    }

    /// A processing run published: during a stop, its journal entry is
    /// left out of the last write.
    pub(super) fn completed(&self, run_id: &str) {
        if self.interrupted.lock().unwrap().is_some() {
            self.completed.lock().unwrap().insert(run_id.to_string());
        }
    }

    /// Whether a processing run published after the stop began.
    pub(super) fn completed_during_stop(&self, run_id: &str) -> bool {
        self.completed.lock().unwrap().contains(run_id)
    }
}

/// A stack build or processing run as it was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournaledStackJob {
    Mono {
        database_id: String,
        project_id: i32,
        automatic: bool,
        request: Box<StackPreviewRequest>,
        /// One build per channel (Build stacks): a request still being
        /// prepared, which becomes its channels' builds.
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        split: bool,
        /// The id the browser follows: the build's, or the one a request
        /// still being prepared has. It comes back under this id, so a page
        /// left open finds it again.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
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
        request: Box<StackColorRequest>,
        /// The id the browser follows: the composition's, or the one a
        /// request still being prepared has. It comes back under this id.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        job_id: Option<String>,
        #[serde(default)]
        attempts: u32,
        #[serde(default)]
        running: bool,
    },
    /// A stretch, deconvolution or RC-Astro run on one channel of a mono
    /// stack. It comes back under the id the browser polls with, so a page
    /// left open finds it again.
    Processing {
        database_id: String,
        project_id: i32,
        /// The stack build and channel group it processes.
        job_id: String,
        group_index: usize,
        /// The stack's revision when it was asked for. A rebuilt stack is not
        /// processed with settings chosen for the old one.
        revision: String,
        /// The run's id: the stretch id the browser polls with.
        run_id: String,
        request: Box<super::stretch::StackViewProcessingRequest>,
        /// Re-applied by the server after the stack refreshed, not asked
        /// for by a person.
        #[serde(default)]
        automatic: bool,
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

impl PartialEq for super::stretch::StackViewProcessingRequest {
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
            request: Box::new(request),
            split: false,
            job_id: Some(job.job_id.clone()),
            attempts: 0,
            running: false,
        }
    }

    /// A mono build as a person asked for it, before it is prepared: the
    /// method is the one the server has when it is prepared.
    pub fn requested(
        database_id: &str,
        project_id: i32,
        request: &StackPreviewRequest,
        split: bool,
        job_id: &str,
        automatic: bool,
    ) -> Self {
        Self::Mono {
            database_id: database_id.to_string(),
            project_id,
            automatic,
            request: Box::new(request.clone()),
            split,
            job_id: Some(job_id.to_string()),
            attempts: 0,
            running: false,
        }
    }

    pub fn color(
        database_id: &str,
        project_id: i32,
        request: &StackColorRequest,
        job_id: Option<&str>,
        automatic: bool,
    ) -> Self {
        Self::Color {
            database_id: database_id.to_string(),
            project_id,
            automatic,
            request: Box::new(request.clone()),
            job_id: job_id.map(str::to_string),
            attempts: 0,
            running: false,
        }
    }

    pub fn set_automatic(&mut self, value: bool) {
        match self {
            Self::Mono { automatic, .. }
            | Self::Color { automatic, .. }
            | Self::Processing { automatic, .. } => *automatic = value,
        }
    }

    pub(super) fn set_running(&mut self, value: bool) {
        match self {
            Self::Mono { running, .. }
            | Self::Color { running, .. }
            | Self::Processing { running, .. } => *running = value,
        }
    }

    pub(super) fn automatic(&self) -> bool {
        match self {
            Self::Mono { automatic, .. }
            | Self::Color { automatic, .. }
            | Self::Processing { automatic, .. } => *automatic,
        }
    }

    pub(super) fn is_processing(&self) -> bool {
        matches!(self, Self::Processing { .. })
    }

    /// What it is, for the log.
    fn noun(&self) -> &'static str {
        if self.is_processing() {
            "processing run"
        } else {
            "stack build"
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
            }
            | Self::Processing {
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

/// Read entry by entry, so one that no longer parses after an upgrade, or one
/// of a kind this server does not know, costs only itself.
pub(super) fn read(path: &Path) -> Option<Journal> {
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
    let readable = u64::from(OLDEST_SCHEMA_VERSION)..=u64::from(SCHEMA_VERSION);
    if !version.is_some_and(|version| readable.contains(&version)) {
        tracing::warn!(
            "Ignoring a job journal of schema {version:?} at {}; this server reads \
             {OLDEST_SCHEMA_VERSION} to {SCHEMA_VERSION}",
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

/// Keep the journal current until the server stops, which writes it itself.
pub fn spawn_writer(state: Arc<AppState>) {
    let Some(path) = path_for(&state) else {
        return;
    };
    tokio::spawn(async move {
        let mut last = read(&path).unwrap_or_default();
        loop {
            if !state.shutdown.sleep(WRITE_INTERVAL).await {
                return;
            }
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

/// Why one build or run was not put back.
pub(super) enum NotRestored {
    /// For good: it finished, is already known, or its database is gone.
    Skip(String),
    /// For now: it could not be prepared, perhaps because its disk is not
    /// mounted yet.
    Retry(String),
}

/// False until [`restore`] has put the line back. Until then a poll for a
/// processing run the server does not know may be for one about to return.
static RESTORED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Whether the journal has been restored since the server started.
pub(super) fn restored() -> bool {
    RESTORED.load(std::sync::atomic::Ordering::Relaxed)
}

/// For tests that start work without a journal restore first.
#[cfg(test)]
pub(super) fn mark_restored() {
    RESTORED.store(true, std::sync::atomic::Ordering::Relaxed);
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
            let noun = job.noun();
            let Some(job) = job.clone().after_restart() else {
                tracing::warn!(
                    "A {noun} cut off by {MAX_ATTEMPTS} restarts in a row is not started again"
                );
                // A re-apply given up on here must not come back through
                // the carry its stack's selection remembers.
                super::reapply::give_up(
                    state,
                    &job,
                    &format!("it was cut off by {MAX_ATTEMPTS} restarts in a row"),
                );
                continue;
            };
            let automatic = job.automatic();
            match restore_stack_job(state, job.clone()).await {
                Ok(()) => restored_a_persons_build |= !automatic,
                Err(NotRestored::Skip(reason)) => {
                    tracing::info!("A queued {noun} was not restored: {reason}")
                }
                Err(NotRestored::Retry(reason)) => {
                    tracing::info!("A queued {noun} will be retried: {reason}");
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
    RESTORED.store(true, std::sync::atomic::Ordering::Relaxed);
    // Re-applies that were still settling come back from the selections
    // that remember them.
    {
        let state = Arc::clone(state);
        let _ = tokio::task::spawn_blocking(move || super::reapply::recover(&state)).await;
    }
    if !DEFERRED.lock().unwrap().is_empty() {
        let state = Arc::clone(state);
        tokio::spawn(async move { retry_deferred(state).await });
    }
}

async fn retry_deferred(state: Arc<AppState>) {
    loop {
        // A stop leaves them in the journal for the next start.
        if !state.shutdown.sleep(RETRY_EVERY).await {
            return;
        }
        let pending = std::mem::take(&mut *DEFERRED.lock().unwrap());
        if pending.is_empty() {
            return;
        }
        for (job, since) in pending {
            let noun = job.noun();
            match restore_stack_job(&state, job.clone()).await {
                Ok(()) => {}
                Err(NotRestored::Skip(reason)) => {
                    tracing::info!("A queued {noun} was not restored: {reason}")
                }
                Err(NotRestored::Retry(reason)) if since.elapsed() >= RETRY_FOR => {
                    tracing::warn!("Gave up restoring a queued {noun}: {reason}")
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
            split,
            job_id,
            ..
        } => {
            let ctx = state
                .get_database(&database_id)
                .ok_or_else(|| NotRestored::Skip(format!("database {database_id} is gone")))?;
            if let Some(job_id) = &job_id
                && state.stack_previews.get(job_id).is_some()
            {
                return Err(NotRestored::Skip(format!("{job_id} is already known")));
            }
            // Back in the line as it was asked for, prepared again by the
            // catalog's runner like any build a person asks for. Nothing is
            // read here, so the restore holds up nothing.
            super::build_set::queue(
                state,
                &ctx,
                super::build_set::Asked {
                    project_id,
                    what: super::build_set::What::Mono { request, split },
                    automatic,
                    job_id,
                    retry_until: Some(Instant::now() + RETRY_FOR),
                },
            )
            .map(|_| ())
            .map_err(|error| NotRestored::Retry(error.message()))
        }
        JournaledStackJob::Color {
            database_id,
            project_id,
            automatic,
            request,
            job_id,
            ..
        } => {
            let ctx = state
                .get_database(&database_id)
                .ok_or_else(|| NotRestored::Skip(format!("database {database_id} is gone")))?;
            if let Some(job_id) = &job_id
                && state.stack_previews.get_color(job_id).is_some()
            {
                return Err(NotRestored::Skip(format!("{job_id} is already known")));
            }
            super::build_set::queue(
                state,
                &ctx,
                super::build_set::Asked {
                    project_id,
                    what: super::build_set::What::Color(request),
                    automatic,
                    job_id,
                    retry_until: Some(Instant::now() + RETRY_FOR),
                },
            )
            .map(|_| ())
            .map_err(|error| NotRestored::Retry(error.message()))
        }
        JournaledStackJob::Processing { .. } => super::stretch::restore_run(state, origin).await,
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
            split: false,
            job_id: None,
            attempts,
            running,
        }
    }

    fn processing(running: bool, attempts: u32) -> JournaledStackJob {
        JournaledStackJob::Processing {
            database_id: "db-a".into(),
            project_id: 7,
            job_id: "a".repeat(64),
            group_index: 1,
            revision: "revision-1".into(),
            run_id: "b".repeat(64),
            request: Box::new(
                serde_json::from_value(serde_json::json!({
                    "model": {"type": "identity"},
                    "rc_astro": {"steps": [{"tool": "bxt", "parameters": {"ss": 0.4}}]},
                }))
                .unwrap(),
            ),
            automatic: false,
            attempts,
            running,
        }
    }

    #[test]
    fn processing_runs_round_trip_in_the_line_beside_builds() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reg.jobs.json");
        // The one running first, then the line: a build, a waiting run, and
        // a re-apply the server queued after a refresh.
        let mut reapply = processing(false, 0);
        reapply.set_automatic(true);
        let journal = Journal {
            schema_version: SCHEMA_VERSION,
            stack: vec![
                processing(true, 1),
                mono(false, 0),
                processing(false, 0),
                reapply,
            ],
            ..Default::default()
        };
        write(&path, &journal).unwrap();
        let written: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(written["schema_version"], 3);
        assert_eq!(written["stack"][0]["kind"], "processing");
        assert_eq!(read(&path), Some(journal));
    }

    #[test]
    fn an_old_journal_still_reads_and_an_unknown_kind_costs_only_itself() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("reg.jobs.json");
        let build = serde_json::to_value(mono(true, 1)).unwrap();
        // Written by a server from before processing runs were journaled.
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": 1,
                "stack": [build],
                "automatic": [],
                "wbpp": [],
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read(&path).unwrap().stack, vec![mono(true, 1)]);
        // A kind from a later server is skipped; the rest still reads.
        let run = serde_json::to_value(processing(false, 0)).unwrap();
        std::fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({
                "schema_version": SCHEMA_VERSION,
                "stack": [{"kind": "something_new", "database_id": "db-a"}, run],
            }))
            .unwrap(),
        )
        .unwrap();
        assert_eq!(read(&path).unwrap().stack, vec![processing(false, 0)]);
    }

    #[test]
    fn a_processing_run_cut_off_too_often_is_not_started_again() {
        assert_eq!(
            processing(false, 0).after_restart(),
            Some(processing(false, 0))
        );
        assert_eq!(
            processing(true, 0).after_restart(),
            Some(processing(false, 1))
        );
        assert_eq!(
            processing(true, MAX_ATTEMPTS - 1).after_restart(),
            Some(processing(false, MAX_ATTEMPTS))
        );
        assert_eq!(processing(true, MAX_ATTEMPTS).after_restart(), None);
        // A run waiting in the line was never cut off.
        assert_eq!(
            processing(false, MAX_ATTEMPTS).after_restart(),
            Some(processing(false, MAX_ATTEMPTS))
        );
        // A person's, unless the server re-applied it; one written before
        // re-applies were journaled reads as a person's.
        let mut run = processing(false, 0);
        assert!(!run.automatic());
        run.set_automatic(true);
        assert!(run.automatic());
        assert!(run.is_processing());
        let mut old = serde_json::to_value(processing(false, 0)).unwrap();
        old.as_object_mut().unwrap().remove("automatic");
        let old: JournaledStackJob = serde_json::from_value(old).unwrap();
        assert!(!old.automatic());
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
