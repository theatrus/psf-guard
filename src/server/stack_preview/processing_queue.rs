//! Processing runs on a finished stack: a display stretch, deconvolution or
//! an RC-Astro chain.
//!
//! They share the single stack worker with builds and wait in the same line,
//! so the line decides what runs next and a person can move or remove a
//! waiting run there. While a run waits or runs, the header's activity list
//! shows it with what holds the worker or how far it has got.

use crate::concurrency::Stop;

use serde::{Deserialize, Serialize};

use super::{
    channel_label, mono_activity, StackActivityEntry, StackActivityKind, StackJobState,
    StackPreviewManager,
};

/// What a processing run works on, for the header and the log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessingScope {
    pub database_id: String,
    pub project_id: i32,
    pub target_id: i32,
    pub target_name: String,
    /// The channel's filter, such as `R`.
    pub channel: String,
    /// The exposure band, when the project keeps exposures apart.
    pub exposure: Option<String>,
}

impl ProcessingScope {
    /// `M44 · R`, or `M44 · R · 300 s` with an exposure band.
    pub fn label(&self) -> String {
        let mut label = channel_label(&self.target_name, &self.channel);
        if let Some(exposure) = &self.exposure {
            label.push_str(" · ");
            label.push_str(exposure);
        }
        label
    }
}

/// One processing run, waiting for the worker or running.
#[derive(Debug, Clone)]
pub(super) struct ProcessingRun {
    scope: ProcessingScope,
    operation: String,
    stoppable: bool,
    /// Re-applied by the server after the stack refreshed.
    automatic: bool,
    running: bool,
    created_unix_seconds: i64,
    started_unix_seconds: Option<i64>,
}

/// A processing run as the activity list reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcessingActivity {
    /// What the run does: `BlurXTerminator`, `Deconvolution`, `Stretch`, or
    /// a chain's steps joined, `BlurXTerminator + StarXTerminator`.
    pub operation: String,
    /// The channel it works on, such as `R`.
    pub channel: String,
    /// The step under way while it runs, such as `BlurXTerminator`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// How far the run has got, 0 to 1, when its step can say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fraction: Option<f32>,
    /// What holds the worker while the run waits, such as
    /// `the stack build of M44 · Ha`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub waiting_for: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_unix_seconds: Option<i64>,
    /// Whether Stop can end it while it runs. An RC-Astro tool can be
    /// killed; a stretch or deconvolution finishes in moments.
    pub stoppable: bool,
}

/// Where a waiting run stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ProcessingWait {
    /// Place in the line, 0 next.
    pub position: Option<usize>,
    /// What holds the worker now, when the manager can tell.
    pub holder: Option<String>,
}

impl ProcessingWait {
    /// `Waiting for the stack build of M44 · Ha to finish`.
    pub fn describe(&self) -> String {
        match (&self.holder, self.position) {
            (Some(holder), _) => format!("Waiting for {holder} to finish"),
            (None, Some(position)) if position > 0 => {
                "Waiting for the work ahead of it in the line".to_string()
            }
            _ => "Waiting for the stack worker".to_string(),
        }
    }
}

/// An RC-Astro schema's name without the vendor, as people say it:
/// `RC-Astro BlurXTerminator` is `BlurXTerminator`.
pub(super) fn tool_name(name: &str) -> &str {
    name.strip_prefix("RC-Astro ").unwrap_or(name)
}

/// What a run does, in the order it does it, for the header and the log.
pub(super) fn operation_name<'a>(
    deconvolution: bool,
    tools: impl IntoIterator<Item = &'a str>,
) -> String {
    let mut steps: Vec<&str> = Vec::new();
    if deconvolution {
        steps.push("Deconvolution");
    }
    steps.extend(tools.into_iter().map(tool_name));
    if steps.is_empty() {
        "Stretch".to_string()
    } else {
        steps.join(" + ")
    }
}

impl StackPreviewManager {
    /// Put a processing run at the back of the line, as builds join it, and
    /// in the job journal as it was asked for. Returns its stop flag and
    /// whether it has to wait for the worker.
    pub(super) fn queue_processing(
        &self,
        run_id: &str,
        scope: ProcessingScope,
        operation: String,
        stoppable: bool,
        origin: super::journal::JournaledStackJob,
    ) -> (Stop, bool) {
        let cancel = self.track_job(run_id);
        let automatic = origin.automatic();
        self.remember_origin(run_id, origin);
        self.processing.lock().unwrap().insert(
            run_id.to_string(),
            ProcessingRun {
                scope,
                operation,
                stoppable,
                automatic,
                running: false,
                created_unix_seconds: chrono::Utc::now().timestamp(),
                started_unix_seconds: None,
            },
        );
        // Its place in line until the run itself joins and takes it.
        self.hold_place(run_id, Vec::new());
        let waits = self.line_position(run_id) != Some(0) || self.line.main_busy();
        (cancel, waits)
    }

    /// The run has the worker and starts, unless a stop reached it first.
    /// Read under the list's lock, so a run a stop flags as it takes the
    /// worker never starts.
    pub(super) fn start_processing(&self, run_id: &str, cancel: &Stop) -> bool {
        let mut processing = self.processing.lock().unwrap();
        if cancel.requested() {
            return false;
        }
        if let Some(run) = processing.get_mut(run_id) {
            run.running = true;
            run.started_unix_seconds = Some(chrono::Utc::now().timestamp());
        }
        true
    }

    /// The run is over, however it ended: it leaves the list, the line and
    /// the journal.
    pub(super) fn finish_processing(&self, run_id: &str) {
        self.processing.lock().unwrap().remove(run_id);
        self.release_place(run_id);
        self.forget_job(run_id);
    }

    /// The run published its result. During a server stop this keeps it out
    /// of the journal's last write; at any other time a finished run has
    /// already left the journal.
    pub(super) fn processing_completed(&self, run_id: &str) {
        if self.stopped_for_shutdown.lock().unwrap().is_some() {
            self.completed_during_stop
                .lock()
                .unwrap()
                .insert(run_id.to_string());
        }
    }

    /// The state of a processing run, for the journal: running or queued
    /// while it is listed, completed once it published during a stop.
    pub(super) fn processing_state(&self, run_id: &str) -> Option<StackJobState> {
        if let Some(run) = self.processing.lock().unwrap().get(run_id) {
            return Some(if run.running {
                StackJobState::Running
            } else {
                StackJobState::Queued
            });
        }
        self.completed_during_stop
            .lock()
            .unwrap()
            .contains(run_id)
            .then_some(StackJobState::Completed)
    }

    /// Whether a processing run is the server's re-apply rather than a
    /// person's.
    pub(super) fn is_automatic_processing(&self, run_id: &str) -> bool {
        self.processing
            .lock()
            .unwrap()
            .get(run_id)
            .is_some_and(|run| run.automatic)
    }

    /// Re-applies queued or running, which step aside for a person's work.
    pub(super) fn automatic_processing_ids(&self) -> Vec<String> {
        self.processing
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, run)| run.automatic)
            .map(|(run_id, _)| run_id.clone())
            .collect()
    }

    /// A person asked for the same processing as a queued re-apply: the run
    /// is theirs now, with a person's priority once it runs.
    pub(super) fn adopt_processing(&self, run_id: &str) {
        if let Some(run) = self.processing.lock().unwrap().get_mut(run_id) {
            run.automatic = false;
        }
    }

    /// Whether a processing run is running and cannot be stopped now.
    pub(super) fn processing_unstoppable(&self, run_id: &str) -> bool {
        self.processing
            .lock()
            .unwrap()
            .get(run_id)
            .is_some_and(|run| run.running && !run.stoppable)
    }

    /// Where a waiting run stands, or `None` once it runs or is gone.
    pub(super) fn processing_wait(&self, run_id: &str) -> Option<ProcessingWait> {
        let waiting = self
            .processing
            .lock()
            .unwrap()
            .get(run_id)
            .is_some_and(|run| !run.running);
        waiting.then(|| ProcessingWait {
            position: self.line_position(run_id),
            holder: self.worker_holder(run_id),
        })
    }

    /// The operation and scope of a run, for its log lines.
    pub(super) fn processing_summary(&self, run_id: &str) -> Option<String> {
        self.processing
            .lock()
            .unwrap()
            .get(run_id)
            .map(|run| format!("{} on {}", run.operation, run.scope.label()))
    }

    /// The composition running in the quick lane, beside the stack worker,
    /// in words that fit "Waiting for … to finish".
    pub(super) fn quick_lane_holder(&self) -> Option<String> {
        let color_jobs = self.color_jobs.lock().unwrap();
        color_jobs
            .values()
            .find(|job| job.state == StackJobState::Running && self.is_quick(&job.job_id))
            .map(|job| {
                format!(
                    "the color composition of {}",
                    channel_label(&job.target_name, &job.label)
                )
            })
    }

    /// What holds the stack worker now, in words that fit "Waiting for …
    /// to finish": a running build, color composition, artifact search or
    /// another processing run. `None` when nothing the manager tracks does.
    pub(super) fn worker_holder(&self, except: &str) -> Option<String> {
        {
            let jobs = self.jobs.lock().unwrap();
            if let Some(job) = jobs
                .values()
                .find(|job| job.state == StackJobState::Running && job.job_id != except)
            {
                let kind = if job.automatic {
                    "automatic stack build"
                } else {
                    "stack build"
                };
                return Some(format!("the {kind} of {}", mono_activity(job).label));
            }
        }
        {
            // A composition in the quick lane runs beside the worker, not on it.
            let color_jobs = self.color_jobs.lock().unwrap();
            if let Some(job) = color_jobs.values().find(|job| {
                job.state == StackJobState::Running
                    && job.job_id != except
                    && !self.is_quick(&job.job_id)
            }) {
                return Some(format!(
                    "the color composition of {}",
                    channel_label(&job.target_name, &job.label)
                ));
            }
        }
        {
            let processing = self.processing.lock().unwrap();
            if let Some(run) = processing
                .iter()
                .find(|(run_id, run)| run.running && run_id.as_str() != except)
                .map(|(_, run)| run)
            {
                return Some(format!("{} on {}", run.operation, run.scope.label()));
            }
        }
        let searching = self
            .artifact_jobs
            .lock()
            .unwrap()
            .values()
            .any(|job| job.state == super::artifact::ArtifactSearchState::Running);
        searching.then(|| "an artifact search".to_string())
    }

    /// Every processing run, waiting or running, as activity entries.
    pub(super) fn processing_activity(&self) -> Vec<StackActivityEntry> {
        let runs = self
            .processing
            .lock()
            .unwrap()
            .iter()
            .map(|(run_id, run)| (run_id.clone(), run.clone()))
            .collect::<Vec<_>>();
        runs.into_iter()
            .map(|(run_id, run)| self.processing_entry(&run_id, &run))
            .collect()
    }

    fn processing_entry(&self, run_id: &str, run: &ProcessingRun) -> StackActivityEntry {
        let progress = run
            .running
            .then(|| super::stretch::run_progress(run_id))
            .flatten();
        let stage = progress
            .as_ref()
            .map(|progress| tool_name(&progress.stage).to_string());
        let fraction = progress
            .and_then(|progress| progress.fraction)
            .map(|fraction| fraction.clamp(0.0, 1.0));
        let wait = (!run.running).then(|| ProcessingWait {
            position: self.line_position(run_id),
            holder: self.worker_holder(run_id),
        });
        // A thousandth is finer than any bar draws.
        let (processed_units, total_units) = match fraction {
            Some(fraction) => ((fraction * 1000.0).round() as usize, 1000),
            None => (0, 0),
        };
        let detail = match &wait {
            Some(wait) => wait.describe(),
            None => stage.clone().unwrap_or_else(|| "Starting".to_string()),
        };
        let progress_label = match (&stage, fraction) {
            (Some(stage), Some(fraction)) => {
                Some(format!("{stage} · {}%", (fraction * 100.0).round()))
            }
            (Some(stage), None) => Some(stage.clone()),
            _ => None,
        };
        StackActivityEntry {
            kind: StackActivityKind::Processing,
            job_id: run_id.to_string(),
            database_id: run.scope.database_id.clone(),
            project_id: run.scope.project_id,
            target_id: Some(run.scope.target_id),
            state: if run.running {
                StackJobState::Running
            } else {
                StackJobState::Queued
            },
            label: run.scope.label(),
            detail,
            processed_units,
            total_units,
            created_unix_seconds: run.created_unix_seconds,
            automatic: run.automatic,
            queue_position: None,
            progress_label: run.running.then_some(progress_label).flatten(),
            processing: Some(ProcessingActivity {
                operation: run.operation.clone(),
                channel: run.scope.channel.clone(),
                stage,
                fraction,
                waiting_for: wait.and_then(|wait| wait.holder),
                started_unix_seconds: run.started_unix_seconds,
                stoppable: run.stoppable,
            }),
        }
    }
}

/// A journal entry for a run, for tests that queue one by hand.
#[cfg(test)]
pub(super) fn test_origin(run_id: &str) -> super::journal::JournaledStackJob {
    super::journal::JournaledStackJob::Processing {
        database_id: "db-test".into(),
        project_id: 7,
        job_id: "e".repeat(64),
        group_index: 0,
        revision: "revision".into(),
        run_id: run_id.into(),
        request: Box::new(
            serde_json::from_value(serde_json::json!({"model": {"type": "identity"}})).unwrap(),
        ),
        automatic: false,
        attempts: 0,
        running: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(channel: &str) -> ProcessingScope {
        ProcessingScope {
            database_id: "db-test".into(),
            project_id: 7,
            target_id: 42,
            target_name: "M44".into(),
            channel: channel.into(),
            exposure: None,
        }
    }

    #[test]
    fn a_run_is_named_by_what_it_does() {
        assert_eq!(operation_name(false, []), "Stretch");
        assert_eq!(operation_name(true, []), "Deconvolution");
        assert_eq!(
            operation_name(false, ["RC-Astro BlurXTerminator"]),
            "BlurXTerminator"
        );
        assert_eq!(
            operation_name(
                true,
                ["RC-Astro BlurXTerminator", "RC-Astro StarXTerminator"]
            ),
            "Deconvolution + BlurXTerminator + StarXTerminator"
        );
        let mut banded = scope("R");
        banded.exposure = Some("300 s".into());
        assert_eq!(banded.label(), "M44 · R · 300 s");
    }

    #[test]
    fn waiting_runs_are_listed_in_line_order_and_leave_when_done() {
        let manager = StackPreviewManager::new();
        let busy = manager.line.try_reserve_main().unwrap();
        let (_, waits) = manager.queue_processing(
            &"a".repeat(64),
            scope("R"),
            "BlurXTerminator".into(),
            true,
            test_origin(&"a".repeat(64)),
        );
        assert!(waits, "the worker is busy");
        manager.queue_processing(
            &"b".repeat(64),
            scope("G"),
            "Stretch".into(),
            false,
            test_origin(&"b".repeat(64)),
        );
        let active = manager.active();
        assert_eq!(active.len(), 2);
        assert!(active
            .iter()
            .all(|entry| entry.kind == StackActivityKind::Processing));
        assert_eq!(active[0].label, "M44 · R");
        assert_eq!(active[0].queue_position, Some(0));
        assert_eq!(active[1].label, "M44 · G");
        assert_eq!(active[1].queue_position, Some(1));
        assert_eq!(active[0].state, StackJobState::Queued);
        // Nothing the manager tracks holds the worker, and the run is next.
        assert_eq!(active[0].detail, "Waiting for the stack worker");
        assert_eq!(
            active[1].detail,
            "Waiting for the work ahead of it in the line"
        );

        // A move puts G first, as it would a build.
        assert!(manager.move_in_line(&"b".repeat(64), 0));
        assert_eq!(manager.active()[0].label, "M44 · G");

        // A server stop flags the waiting runs; one flagged as the worker
        // frees up never starts.
        manager.stop_for_shutdown();
        let flag = manager.line.stop_flag(&"a".repeat(64)).unwrap();
        assert!(!manager.start_processing(&"a".repeat(64), &flag));
        assert_eq!(manager.active()[1].state, StackJobState::Queued);

        drop(busy);
        manager.finish_processing(&"a".repeat(64));
        manager.finish_processing(&"b".repeat(64));
        assert!(manager.active().is_empty());
        assert!(manager.line.order().is_empty());
        assert!(!manager.request_cancel(&"a".repeat(64)));
    }

    #[test]
    fn a_running_run_reports_its_stage_and_fraction() {
        let manager = StackPreviewManager::new();
        let run_id = "c".repeat(64);
        let (cancel, _) = manager.queue_processing(
            &run_id,
            scope("R"),
            "BlurXTerminator".into(),
            true,
            test_origin(&run_id),
        );
        manager.release_place(&run_id);
        assert!(manager.start_processing(&run_id, &cancel));
        super::super::stretch::report_progress(&run_id, "RC-Astro BlurXTerminator", 0.42);
        let entry = manager.active().remove(0);
        assert_eq!(entry.state, StackJobState::Running);
        assert_eq!(entry.detail, "BlurXTerminator");
        assert_eq!(
            entry.progress_label.as_deref(),
            Some("BlurXTerminator · 42%")
        );
        assert_eq!((entry.processed_units, entry.total_units), (420, 1000));
        let processing = entry.processing.unwrap();
        assert_eq!(processing.stage.as_deref(), Some("BlurXTerminator"));
        assert_eq!(processing.channel, "R");
        assert!(processing.started_unix_seconds.is_some());
        assert!(processing.waiting_for.is_none());
        // Another run waiting now names this one.
        let other = "d".repeat(64);
        manager.queue_processing(
            &other,
            scope("G"),
            "Stretch".into(),
            false,
            test_origin(&other),
        );
        assert_eq!(
            manager.processing_wait(&other).unwrap().describe(),
            "Waiting for BlurXTerminator on M44 · R to finish"
        );
        super::super::stretch::clear_progress(&run_id);
        manager.finish_processing(&run_id);
        manager.finish_processing(&other);
    }
}
