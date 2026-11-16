//! RC-Astro license checks a person started: on this server, or on one
//! connected worker.
//!
//! `rc-astro license --json` makes an online check, and RC-Astro asks hosts
//! not to run it routinely, so a check runs only when a person clicks. It
//! is a tracked job: the request that starts it returns at once, the job
//! has one runner thread and a row in the job line, it can be stopped, and
//! the page reads its state until the result is in. A second click for the
//! same computer while a check runs joins that check.
//!
//! A worker's check is not work for a slot. The worker asks for side
//! requests on a connection of its own (`…/sessions/{session}/side`), so a
//! worker with every slot busy still answers, and no running work is
//! stopped for it. A request is kept for the worker's token and name, so a
//! worker that reconnects while a check waits still gets it.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::board::WorkerIdentity;
use crate::remote_work::protocol::{SideAnswer, SideKind, SideRequest};
use crate::remote_work::rc_astro::{self, LicenseCheck};
use crate::server::state::AppState;

/// How long the server waits for a worker's answer: the check's own limit
/// on the worker, and time for the worker to pick the request up.
pub const WORKER_ANSWER_LIMIT: Duration =
    Duration::from_secs(rc_astro::LICENSE_CHECK_LIMIT.as_secs() + 30);

/// How many finished checks are kept for their pages to read.
const KEPT: usize = 32;

/// A check's state, for the page that started it. It holds the RC-Astro
/// account's address, so it goes only to accounts that may write, and
/// never into the job line.
#[derive(Debug, Clone, Serialize)]
pub struct CheckStatus {
    /// What the page polls with.
    pub key: String,
    /// The computer checked: "this server", or a worker's name.
    pub computer: String,
    pub running: bool,
    pub started_at: u64,
    /// The report, once the check has ended with one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub check: Option<LicenseCheck>,
    /// Why there is no check to show: the worker did not answer, or
    /// rc-astro was not found.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub problem: Option<String>,
}

/// A running check, for the job line: no result, and no account.
#[derive(Debug, Clone)]
pub struct RunningCheck {
    pub key: String,
    pub computer: String,
    pub started_at: u64,
}

struct Entry {
    computer: String,
    started: Instant,
    started_at: u64,
    stop: crate::concurrency::Stop,
    /// The worker asked, and the request it has not picked up yet.
    worker: Option<(WorkerIdentity, bool)>,
    outcome: Option<Result<LicenseCheck, String>>,
}

impl Entry {
    fn status(&self, key: &str) -> CheckStatus {
        let (check, problem) = match &self.outcome {
            Some(Ok(check)) => (Some(check.clone()), None),
            Some(Err(problem)) => (None, Some(problem.clone())),
            None => (None, None),
        };
        CheckStatus {
            key: key.to_string(),
            computer: self.computer.clone(),
            running: self.outcome.is_none(),
            started_at: self.started_at,
            check,
            problem,
        }
    }
}

/// The license checks under way and the last ones that ended.
#[derive(Default)]
pub struct LicenseChecks {
    entries: Mutex<HashMap<String, Entry>>,
    changed: Condvar,
}

fn server_key() -> String {
    "server".to_string()
}

fn worker_key(worker: &WorkerIdentity) -> String {
    // Hex of the pair, so the key is one path segment whatever the name.
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{}\n{}", worker.token_id, worker.name));
    format!("worker-{}", &crate::remote_work::hex(&digest)[..24])
}

impl LicenseChecks {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// A check's state, by its key.
    pub fn status(&self, key: &str) -> Option<CheckStatus> {
        self.lock().get(key).map(|entry| entry.status(key))
    }

    /// The checks running now, for the job line.
    pub fn running(&self) -> Vec<RunningCheck> {
        let mut running: Vec<RunningCheck> = self
            .lock()
            .iter()
            .filter(|(_, entry)| entry.outcome.is_none())
            .map(|(key, entry)| RunningCheck {
                key: key.clone(),
                computer: entry.computer.clone(),
                started_at: entry.started_at,
            })
            .collect();
        running.sort_by_key(|check| check.started_at);
        running
    }

    /// Stop a running check. False when none runs under `key`.
    pub fn stop(&self, key: &str) -> bool {
        let entries = self.lock();
        let Some(entry) = entries.get(key).filter(|entry| entry.outcome.is_none()) else {
            return false;
        };
        entry.stop.request();
        self.changed.notify_all();
        true
    }

    /// Put a new running entry under `key`, or return the status of the
    /// check already running there: a second click joins it, and makes no
    /// second online check.
    fn begin(
        &self,
        key: &str,
        computer: &str,
        worker: Option<WorkerIdentity>,
    ) -> Result<crate::concurrency::Stop, Box<CheckStatus>> {
        let mut entries = self.lock();
        if let Some(entry) = entries.get(key).filter(|entry| entry.outcome.is_none()) {
            return Err(Box::new(entry.status(key)));
        }
        // Drop the oldest finished checks beyond the ones kept.
        let mut finished: Vec<(String, Instant)> = entries
            .iter()
            .filter(|(_, entry)| entry.outcome.is_some())
            .map(|(key, entry)| (key.clone(), entry.started))
            .collect();
        finished.sort_by_key(|(_, started)| *started);
        while finished.len() >= KEPT {
            entries.remove(&finished.remove(0).0);
        }
        let stop = crate::concurrency::Stop::new();
        entries.insert(
            key.to_string(),
            Entry {
                computer: computer.to_string(),
                started: Instant::now(),
                started_at: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs(),
                stop: stop.clone(),
                worker: worker.map(|worker| (worker, false)),
                outcome: None,
            },
        );
        Ok(stop)
    }

    fn finish(&self, key: &str, outcome: Result<LicenseCheck, String>) {
        if let Some(entry) = self.lock().get_mut(key)
            && entry.outcome.is_none()
        {
            entry.outcome = Some(outcome);
        }
        self.changed.notify_all();
    }

    /// Start a check of this server's rc-astro and return at once. The
    /// runner thread holds the interactive guard: a person started it.
    pub fn start_local(self: &Arc<Self>, state: &Arc<AppState>) -> CheckStatus {
        let key = server_key();
        let stop = match self.begin(&key, "this server", None) {
            Ok(stop) => stop,
            Err(running) => return *running,
        };
        let checks = Arc::clone(self);
        let guard = state.begin_named_interactive_job("an RC-Astro license check");
        let thread_key = key.clone();
        let spawned = std::thread::Builder::new()
            .name("rc-astro-license".into())
            .spawn(move || {
                let _guard = guard;
                let outcome = match rc_astro::locate(None) {
                    Some(cli) => Ok(rc_astro::check_license(
                        &cli,
                        rc_astro::LICENSE_CHECK_LIMIT,
                        &|| stop.requested(),
                    )),
                    None => Err(format!(
                        "rc-astro was not found on this server (looked in {}).",
                        rc_astro::searched_places()
                    )),
                };
                checks.finish(&thread_key, outcome);
            });
        if let Err(error) = spawned {
            self.finish(&key, Err(format!("The check could not start: {error}.")));
        }
        self.status(&key).expect("the check was just added")
    }

    /// Ask one worker for a check and return at once. The request waits
    /// for the worker to pick it up on its side connection; the runner
    /// thread ends the check when the worker has not answered in time.
    pub fn start_worker(self: &Arc<Self>, worker: WorkerIdentity) -> CheckStatus {
        let key = worker_key(&worker);
        let stop = match self.begin(&key, &worker.name.clone(), Some(worker.clone())) {
            Ok(stop) => stop,
            Err(running) => return *running,
        };
        let checks = Arc::clone(self);
        let thread_key = key.clone();
        let spawned = std::thread::Builder::new()
            .name("rc-astro-license".into())
            .spawn(move || checks.wait_for_worker(&thread_key, &worker.name, &stop));
        if let Err(error) = spawned {
            self.finish(&key, Err(format!("The check could not start: {error}.")));
        }
        self.status(&key).expect("the check was just added")
    }

    fn wait_for_worker(&self, key: &str, name: &str, stop: &crate::concurrency::Stop) {
        let deadline = Instant::now() + WORKER_ANSWER_LIMIT;
        let mut entries = self.lock();
        loop {
            match entries.get(key) {
                Some(entry) if entry.outcome.is_none() => {}
                _ => return,
            }
            let outcome = if stop.requested() {
                Some("The license check was stopped.".to_string())
            } else if Instant::now() >= deadline {
                Some(format!(
                    "{name} did not answer within {} seconds. Check that the worker is connected \
                     and try again, or run `psf-guard-cli rc-astro license` on that computer.",
                    WORKER_ANSWER_LIMIT.as_secs()
                ))
            } else {
                None
            };
            if let Some(problem) = outcome {
                if let Some(entry) = entries.get_mut(key) {
                    entry.outcome = Some(Err(problem));
                }
                self.changed.notify_all();
                return;
            }
            entries = self
                .changed
                .wait_timeout(entries, Duration::from_millis(250))
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// The request waiting for `worker`, when one is: the worker's side
    /// connection asks. Each request is handed over once.
    pub fn take_for(&self, worker: &WorkerIdentity) -> Option<SideRequest> {
        let mut entries = self.lock();
        let (key, entry) = entries.iter_mut().find(|(_, entry)| {
            entry.outcome.is_none()
                && entry.worker.as_ref().is_some_and(|(asked, sent)| {
                    !sent && asked.token_id == worker.token_id && asked.name == worker.name
                })
        })?;
        if let Some((_, sent)) = &mut entry.worker {
            *sent = true;
        }
        Some(SideRequest {
            id: key.clone(),
            kind: SideKind::RcAstroLicense,
        })
    }

    /// The worker's answer to request `id`. An answer from another worker
    /// than the one asked, or to a check that has ended, is dropped.
    pub fn answer(&self, worker: &WorkerIdentity, id: &str, answer: SideAnswer) {
        let asked = self.lock().get(id).is_some_and(|entry| {
            entry.outcome.is_none()
                && entry.worker.as_ref().is_some_and(|(asked, _)| {
                    asked.token_id == worker.token_id && asked.name == worker.name
                })
        });
        if !asked {
            return;
        }
        let outcome = match answer.error {
            Some(error) => Err(format!("{}: {error}", worker.name)),
            None => serde_json::from_value::<LicenseCheck>(answer.result).map_err(|error| {
                format!(
                    "{} sent back a report that could not be read: {error}.",
                    worker.name
                )
            }),
        };
        self.finish(id, outcome);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(name: &str) -> WorkerIdentity {
        WorkerIdentity {
            token_id: "token".into(),
            name: name.into(),
            local: false,
        }
    }

    fn report(computer: &str) -> SideAnswer {
        SideAnswer {
            result: serde_json::to_value(LicenseCheck {
                executable: "/opt/rc-astro/rc-astro".into(),
                report: Some(rc_astro::LicenseReport {
                    computer: computer.into(),
                    ..Default::default()
                }),
                ..Default::default()
            })
            .unwrap(),
            error: None,
        }
    }

    #[test]
    fn a_second_click_joins_the_check_and_only_its_worker_is_asked_once() {
        let checks = Arc::new(LicenseChecks::default());
        let started = checks.start_worker(worker("attic"));
        assert!(started.running);
        assert_eq!(started.computer, "attic");
        // A second click while it runs starts no second check.
        let again = checks.start_worker(worker("attic"));
        assert_eq!(again.key, started.key);
        assert_eq!(again.started_at, started.started_at);
        assert_eq!(checks.running().len(), 1);
        // Another worker is a check of its own, and gets its own request.
        let other = checks.start_worker(worker("studio"));
        assert_ne!(other.key, started.key);

        // Only the worker asked picks the request up, and once. A worker
        // that registered again is the same worker: its token and name.
        assert!(checks.take_for(&worker("nobody")).is_none());
        let request = checks.take_for(&worker("attic")).unwrap();
        assert_eq!(request.id, started.key);
        assert_eq!(request.kind, SideKind::RcAstroLicense);
        assert!(checks.take_for(&worker("attic")).is_none());

        // Another worker's answer is dropped; the asked worker's ends it.
        checks.answer(&worker("studio"), &started.key, report("wrong@studio"));
        assert!(checks.status(&started.key).unwrap().running);
        checks.answer(&worker("attic"), &started.key, report("svc@attic"));
        let done = checks.status(&started.key).unwrap();
        assert!(!done.running);
        assert_eq!(done.check.unwrap().report.unwrap().computer, "svc@attic");
        // The other check still runs, and is the one row in the job line.
        let running = checks.running();
        assert_eq!(running.len(), 1);
        assert_eq!(running[0].computer, "studio");
        // After it ended, a click starts a new check.
        assert!(checks.start_worker(worker("attic")).running);
    }

    #[test]
    fn stop_ends_a_check_and_a_worker_s_refusal_is_its_problem() {
        let checks = Arc::new(LicenseChecks::default());
        let started = checks.start_worker(worker("attic"));
        assert!(checks.stop(&started.key));
        let ended = (0..200).find_map(|_| {
            std::thread::sleep(Duration::from_millis(10));
            checks.status(&started.key).filter(|status| !status.running)
        });
        assert_eq!(
            ended.unwrap().problem.as_deref(),
            Some("The license check was stopped.")
        );
        assert!(!checks.stop(&started.key));
        assert!(!checks.stop("nothing"));

        let started = checks.start_worker(worker("attic"));
        checks.take_for(&worker("attic")).unwrap();
        checks.answer(
            &worker("attic"),
            &started.key,
            SideAnswer {
                result: serde_json::Value::Null,
                error: Some("rc-astro is not installed on this worker".into()),
            },
        );
        assert_eq!(
            checks.status(&started.key).unwrap().problem.as_deref(),
            Some("attic: rc-astro is not installed on this worker")
        );
    }
}
