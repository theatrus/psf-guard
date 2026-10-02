//! The job journal: what was queued or running, kept on disk so a restart
//! picks it up again.
//!
//! Stack builds (mono and color) in the order they would run, automatic
//! refreshes still settling, and the WBPP line with any run a restart cut
//! off. A writer compares a fresh snapshot with the last one it wrote every
//! few seconds and replaces the file atomically when they differ; nothing
//! else has to remember to save it. The file sits beside the database
//! registry, so a server without one keeps no journal.

use super::automatic::RefreshReason;
use super::color::StackColorRequest;
use super::{StackPreviewJob, StackPreviewRequest};
use crate::server::state::AppState;
use crate::server::wbpp_run::StartWbppRunRequest;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const SCHEMA_VERSION: u32 = 1;
/// How often the writer looks for changes.
const WRITE_INTERVAL: Duration = Duration::from_secs(3);

/// A stack build as it was asked for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournaledStackJob {
    Mono {
        database_id: String,
        project_id: i32,
        automatic: bool,
        request: StackPreviewRequest,
    },
    Color {
        database_id: String,
        project_id: i32,
        automatic: bool,
        request: StackColorRequest,
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
        }
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
}

impl PartialEq for JournaledWbppRun {
    fn eq(&self, other: &Self) -> bool {
        self.db_id == other.db_id
            && serde_json::to_value(&self.request).ok() == serde_json::to_value(&other.request).ok()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    pub schema_version: u32,
    /// Running first, then the line.
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
        if let Some(request) = ctx.wbpp_run.read().unwrap().running_request() {
            wbpp.push(JournaledWbppRun {
                db_id: ctx.id.clone(),
                request,
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
            }),
    );
    let mut automatic = state.auto_stacks.journal_entries();
    automatic.sort_by(|left, right| {
        left.due_unix
            .cmp(&right.due_unix)
            .then_with(|| left.database_id.cmp(&right.database_id))
            .then_with(|| left.project_id.cmp(&right.project_id))
    });
    Journal {
        schema_version: SCHEMA_VERSION,
        stack: state.stack_previews.journal_entries(),
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

fn read(path: &Path) -> Option<Journal> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice::<Journal>(&bytes) {
        Ok(journal) if journal.schema_version == SCHEMA_VERSION => Some(journal),
        Ok(_) => None,
        Err(error) => {
            tracing::warn!(
                "Ignoring an unreadable job journal at {}: {error}",
                path.display()
            );
            None
        }
    }
}

/// Keep the journal current until the process ends.
pub fn spawn_writer(state: Arc<AppState>) {
    let Some(registry) = state.registry_path.read().unwrap().clone() else {
        return;
    };
    let path = journal_path(&registry);
    tokio::spawn(async move {
        // Restore wrote nothing yet; start from what is on disk so an
        // unchanged queue is not rewritten.
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

/// Put back what the journal kept: stack builds in their order, settling
/// refreshes at their due times, and the WBPP line with a cut-off run first.
/// Each entry is prepared again from today's catalog, so one whose project
/// or frames are gone is skipped with a note in the log.
pub async fn restore(state: &Arc<AppState>) {
    let Some(registry) = state.registry_path.read().unwrap().clone() else {
        return;
    };
    let Some(journal) = read(&journal_path(&registry)) else {
        return;
    };
    tracing::info!(
        stack = journal.stack.len(),
        automatic = journal.automatic.len(),
        wbpp = journal.wbpp.len(),
        "Restoring the job queue from before the restart"
    );
    for refresh in &journal.automatic {
        state.auto_stacks.restore(refresh);
    }
    for job in journal.stack {
        if let Err(reason) = restore_stack_job(state, job).await {
            tracing::info!("A queued stack build was not restored: {reason}");
        }
    }
    crate::server::wbpp_run::restore_queue(
        state,
        journal
            .wbpp
            .into_iter()
            .map(|run| (run.db_id, run.request))
            .collect(),
    );
}

async fn restore_stack_job(state: &Arc<AppState>, job: JournaledStackJob) -> Result<(), String> {
    match job {
        JournaledStackJob::Mono {
            database_id,
            project_id,
            automatic,
            request,
        } => {
            let ctx = state
                .get_database(&database_id)
                .ok_or_else(|| format!("database {database_id} is gone"))?;
            let ctx_for_prepare = Arc::clone(&ctx);
            let request_for_prepare = request.clone();
            let mut prepared = tokio::task::spawn_blocking(move || {
                super::prepare_job(&ctx_for_prepare, project_id, &request_for_prepare)
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("{error:?}"))?;
            let job_id = prepared.public.job_id.clone();
            if state.stack_previews.get(&job_id).is_some() {
                return Err(format!("{job_id} is already known"));
            }
            if let Ok(bytes) = std::fs::read(super::manifest_path(&prepared.cache_root, &job_id))
                && let Ok(existing) = serde_json::from_slice::<StackPreviewJob>(&bytes)
                && existing.state == super::StackJobState::Completed
            {
                return Err(format!("{job_id} finished before the restart"));
            }
            prepared.public.automatic = automatic;
            if !state.stack_previews.insert(prepared.public.clone()) {
                return Err("too many stack jobs are active".into());
            }
            let origin = JournaledStackJob::mono(
                &database_id,
                project_id,
                &request,
                &prepared.public,
                automatic,
            );
            super::enqueue_job(Arc::clone(state), prepared, origin);
            Ok(())
        }
        JournaledStackJob::Color {
            database_id,
            project_id,
            automatic,
            request,
        } => {
            let ctx = state
                .get_database(&database_id)
                .ok_or_else(|| format!("database {database_id} is gone"))?;
            let ctx_for_prepare = Arc::clone(&ctx);
            let request_for_prepare = request.clone();
            let mut prepared = tokio::task::spawn_blocking(move || {
                super::color::prepare_color_job(&ctx_for_prepare, project_id, &request_for_prepare)
            })
            .await
            .map_err(|error| error.to_string())?
            .map_err(|error| format!("{error:?}"))?;
            let job_id = prepared.public.job_id.clone();
            if state.stack_previews.get_color(&job_id).is_some() {
                return Err(format!("{job_id} is already known"));
            }
            prepared.public.automatic = automatic;
            if !state.stack_previews.insert_color(prepared.public.clone()) {
                return Err("too many color jobs are active".into());
            }
            let origin = JournaledStackJob::Color {
                database_id,
                project_id,
                automatic,
                request,
            };
            super::color::enqueue_color_job(Arc::clone(state), prepared, origin);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            stack: vec![JournaledStackJob::Mono {
                database_id: "db-a".into(),
                project_id: 7,
                automatic: true,
                request: serde_json::from_value(serde_json::json!({
                    "image_ids": [1, 2, 3],
                    "method": {"final_pass": "draft"}
                }))
                .unwrap(),
            }],
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
            }],
        };
        write(&path, &journal).unwrap();
        assert_eq!(read(&path), Some(journal));
        // An empty queue leaves no file behind.
        write(&path, &Journal::default()).unwrap();
        assert!(!path.exists());
        assert_eq!(read(&path), None);
    }
}
