//! The task board: tasks waiting for a worker, the workers connected to
//! this server, and the lease each running task is under.
//!
//! Work on this server submits a task and waits on its [`Ticket`]. A worker
//! claims the first queued task it can run and holds a lease on it. Each
//! progress report renews the lease; a lease that runs out puts the task
//! back in the queue, and a task that loses [`MAX_ATTEMPTS`] leases fails.
//! The server checks each output's size and digest before the waiting work
//! reads it. A late answer under an old lease is refused.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::remote_work::protocol::{
    Assignment, Completion, ExchangeProbe, FileAccess, FileMode, InputFile, Offers, OutputFile,
    ProbeAnswer, ProbeResult, Registration, TaskKind,
};

/// How long a lease lasts without a progress report. A worker reports six
/// times a lease while a task runs.
pub const LEASE: Duration = Duration::from_secs(60);
/// How long the server holds a claim open before it answers that no work
/// is ready.
pub const POLL: Duration = Duration::from_secs(25);
/// A worker heard from this recently counts as connected.
const ONLINE: Duration = Duration::from_secs(90);
/// A worker not heard from for this long leaves the list.
const FORGOTTEN: Duration = Duration::from_secs(3600);
/// Leases a task may lose before it fails.
pub const MAX_ATTEMPTS: u32 = 3;
/// How long a queued task waits when no connected worker can run it, for
/// one to connect, before it fails.
const NO_WORKER_GRACE: Duration = Duration::from_secs(30);
/// How long a stop waits for this server's worker to stop the task.
const DRAIN_LIMIT: Duration = Duration::from_secs(30);

/// What a worker must offer to run a task.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskNeeds {
    /// This RC-Astro tool, at this CLI and model version, licensed.
    RcAstro {
        tool: String,
        name: String,
        cli_version: String,
        ml_version: Option<i64>,
    },
}

impl TaskNeeds {
    fn met_by(&self, offers: &Offers) -> bool {
        match self {
            TaskNeeds::RcAstro {
                tool,
                cli_version,
                ml_version,
                ..
            } => {
                offers.work.contains(&TaskKind::RcAstroStep)
                    && offers.rc_astro.iter().any(|schema| {
                        schema.key == *tool
                            && schema.licensed
                            && schema.cli_version == *cli_version
                            && schema.ml_version == *ml_version
                    })
            }
        }
    }

    /// What the task needs, as a person reads it.
    fn describe(&self) -> String {
        match self {
            TaskNeeds::RcAstro {
                name, cli_version, ..
            } => format!("{name} {cli_version}"),
        }
    }
}

/// A task to put on the board.
pub struct NewTask {
    pub kind: TaskKind,
    /// What the task is, for the worker's log and Settings › Workers.
    pub label: String,
    /// A person waits for it, so it goes ahead of automatic work.
    pub person: bool,
    pub needs: TaskNeeds,
    pub spec: serde_json::Value,
    /// The files it reads, by name.
    pub inputs: Vec<(String, PathBuf)>,
    /// The files it writes, by name, and whether each is required.
    pub outputs: Vec<(String, bool)>,
}

/// A connected worker, as the board knows it.
#[derive(Debug, Clone)]
struct Session {
    token_id: String,
    name: String,
    version: String,
    offers: Offers,
    files: FileAccess,
    /// Whether the worker read the exchange probe and wrote its reply, so
    /// task files go to it by path.
    exchange_shared: bool,
    /// Why task files go to it over HTTP although it asked for shared mode.
    exchange_problem: Option<String>,
    probe: Option<Probe>,
    connected_at: i64,
    last_seen: Instant,
    /// The task it holds a lease on.
    task: Option<String>,
    /// This server's own worker.
    local: bool,
}

#[derive(Debug, Clone)]
struct Probe {
    path: PathBuf,
    reply_path: PathBuf,
    digest: String,
}

#[derive(Debug)]
struct Task {
    kind: TaskKind,
    label: String,
    person: bool,
    needs: TaskNeeds,
    spec: serde_json::Value,
    inputs: Vec<(String, PathBuf, u64)>,
    outputs: Vec<(String, bool)>,
    dir: PathBuf,
    seq: u64,
    state: TaskState,
    attempts: u32,
    fraction: f32,
    /// Since when no connected worker could run it, while it is queued.
    unmatched_since: Option<Instant>,
}

#[derive(Debug)]
enum TaskState {
    Queued,
    Leased {
        session: String,
        lease: String,
        expires: Instant,
    },
    /// Its outputs are being checked, and the worker has nothing more to
    /// report.
    Checking {
        session: String,
    },
    Finished(Result<Finished, String>),
}

/// What a finished task made. Its output files stay until the ticket is
/// dropped.
#[derive(Debug, Clone)]
pub struct Finished {
    pub result: serde_json::Value,
    /// The worker that ran it.
    pub worker: String,
    /// Each output it wrote, by name.
    pub outputs: BTreeMap<String, PathBuf>,
}

/// Why a worker's request about a task was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The session is unknown: the worker registers again.
    UnknownSession,
    /// The task is gone, or the lease is not this worker's current one: the
    /// worker drops the task.
    NotCurrent,
    /// The request itself was wrong.
    Invalid(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::UnknownSession => {
                formatter.write_str("This worker session is not known; register again")
            }
            Refusal::NotCurrent => {
                formatter.write_str("The server no longer wants this task from this worker")
            }
            Refusal::Invalid(message) => formatter.write_str(message),
        }
    }
}

#[derive(Default)]
struct Inner {
    sessions: HashMap<String, Session>,
    tasks: HashMap<String, Task>,
    /// Tasks taken off the board while this server's worker ran them, until
    /// that worker has stopped them.
    draining: std::collections::HashSet<String>,
    next_seq: u64,
    closed: bool,
}

/// The board. Shared by the workers' routes, this server's own worker, and
/// the work that waits on tasks.
pub struct Board {
    inner: Mutex<Inner>,
    /// Woken on every change, for work waiting on a ticket and this server's
    /// worker.
    changed: Condvar,
    /// Woken on every change, for claims waiting over HTTP.
    claims: tokio::sync::Notify,
    exchange: Mutex<PathBuf>,
    timings: Timings,
}

/// How long leases last and how long a task waits for a worker to connect.
#[derive(Debug, Clone, Copy)]
pub struct Timings {
    pub lease: Duration,
    pub no_worker_grace: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            lease: LEASE,
            no_worker_grace: NO_WORKER_GRACE,
        }
    }
}

/// A connected worker, for Settings › Workers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionSummary {
    pub session: String,
    pub token_id: String,
    pub name: String,
    pub version: String,
    pub local: bool,
    pub connected: bool,
    pub connected_at: i64,
    pub seconds_since_seen: u64,
    pub threads: usize,
    /// The RC-Astro tools it offers, as "StarXTerminator 2.6.6".
    pub rc_astro: Vec<String>,
    pub file_mode: FileMode,
    /// Whether task files reach it by path.
    pub exchange_shared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange_problem: Option<String>,
    /// What it runs now, with how much is done.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub running: Option<RunningSummary>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct RunningSummary {
    pub label: String,
    pub fraction: f32,
}

/// Tasks waiting for a worker, for Settings › Workers.
#[derive(Debug, Clone, serde::Serialize)]
pub struct QueuedSummary {
    pub label: String,
    pub needs: String,
}

impl Default for Board {
    fn default() -> Self {
        Self::new(std::env::temp_dir().join(format!("psf-guard-workers-{}", std::process::id())))
    }
}

impl Board {
    pub fn new(exchange: PathBuf) -> Self {
        Self::with_timings(exchange, Timings::default())
    }

    pub fn with_timings(exchange: PathBuf, timings: Timings) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            changed: Condvar::new(),
            claims: tokio::sync::Notify::new(),
            exchange: Mutex::new(exchange),
            timings,
        }
    }

    /// How long a lease lasts without a progress report.
    pub fn lease(&self) -> Duration {
        self.timings.lease
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn notify(&self) {
        self.changed.notify_all();
        self.claims.notify_waiters();
    }

    /// The folder task files go through.
    pub fn exchange(&self) -> PathBuf {
        self.exchange
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Move the exchange folder. Tasks already submitted keep theirs.
    pub fn set_exchange(&self, folder: PathBuf) {
        *self
            .exchange
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = folder;
    }

    /// Stop handing out work: claims answer at once, and this server's
    /// worker ends.
    pub fn close(&self) {
        self.lock().closed = true;
        self.notify();
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// Register a worker under `token_id`. A worker in shared mode gets a
    /// probe to read, unless it is this server's own.
    pub fn register(&self, token_id: &str, registration: Registration, local: bool) -> String {
        let session = uuid::Uuid::new_v4().to_string();
        let probe = (registration.files.mode == FileMode::Shared && !local)
            .then(|| self.write_probe(&session))
            .and_then(|probe| match probe {
                Ok(probe) => Some(probe),
                Err(error) => {
                    tracing::warn!("Could not write the worker exchange probe: {error}");
                    None
                }
            });
        let mut inner = self.lock();
        // A worker that registers again under the same name, such as after a
        // restart, replaces its old session: its task goes back in the queue.
        let replaced = inner
            .sessions
            .iter()
            .filter(|(_, old)| old.token_id == token_id && old.name == registration.name)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for old in replaced {
            remove_session(&mut inner, &old, "it connected again");
        }
        inner.sessions.insert(
            session.clone(),
            Session {
                token_id: token_id.to_string(),
                name: registration.name,
                version: registration.version,
                offers: registration.offers,
                exchange_shared: local,
                exchange_problem: None,
                files: registration.files,
                probe,
                connected_at: crate::auth_registry::unix_now(),
                last_seen: Instant::now(),
                task: None,
                local,
            },
        );
        drop(inner);
        self.notify();
        session
    }

    fn write_probe(&self, session: &str) -> std::io::Result<Probe> {
        let folder = self.exchange().join("probes");
        std::fs::create_dir_all(&folder)?;
        let path = folder.join(format!("{session}.probe"));
        let reply_path = folder.join(format!("{session}.reply"));
        let random: [u8; 32] = rand::random();
        let content = crate::remote_work::hex(&random);
        std::fs::write(&path, &content)?;
        let digest = {
            use sha2::{Digest, Sha256};
            crate::remote_work::hex(&Sha256::digest(content.as_bytes()))
        };
        Ok(Probe {
            path,
            reply_path,
            digest,
        })
    }

    /// The probe a session reads, for its registration's answer.
    pub fn probe_of(&self, session: &str) -> Option<ExchangeProbe> {
        let inner = self.lock();
        let probe = inner.sessions.get(session)?.probe.as_ref()?;
        Some(ExchangeProbe {
            path: probe.path.display().to_string(),
            reply_path: probe.reply_path.display().to_string(),
        })
    }

    /// Check a worker's answer to its probe: the digest of what it read,
    /// and the reply it wrote beside the probe.
    pub fn answer_probe(&self, session: &str, answer: ProbeAnswer) -> Result<ProbeResult, Refusal> {
        let probe = {
            let inner = self.lock();
            let found = inner.sessions.get(session).ok_or(Refusal::UnknownSession)?;
            found
                .probe
                .clone()
                .ok_or_else(|| Refusal::Invalid("This worker was sent no probe".into()))?
        };
        let reason = match answer.digest {
            None => Some(answer.problem.unwrap_or_else(|| {
                "The worker could not read the server's exchange folder".into()
            })),
            Some(digest) if digest != probe.digest => {
                Some("The file the worker read is not the probe the server wrote".into())
            }
            Some(_) => match std::fs::read(&probe.reply_path) {
                Ok(reply)
                    if crate::remote_work::hex(&{
                        use sha2::{Digest, Sha256};
                        Sha256::digest(&reply)
                    }) == probe.digest =>
                {
                    None
                }
                Ok(_) => Some("The reply the worker wrote does not match the probe".into()),
                Err(_) => {
                    Some("The worker's reply did not reach the server's exchange folder".into())
                }
            },
        };
        let _ = std::fs::remove_file(&probe.path);
        let _ = std::fs::remove_file(&probe.reply_path);
        let mut inner = self.lock();
        let found = inner
            .sessions
            .get_mut(session)
            .ok_or(Refusal::UnknownSession)?;
        found.probe = None;
        found.exchange_shared = reason.is_none();
        found.exchange_problem = reason.clone();
        found.last_seen = Instant::now();
        Ok(ProbeResult {
            shared: reason.is_none(),
            reason,
        })
    }

    /// Replace what a session offers, such as after this server's rc-astro
    /// was installed.
    pub fn update_offers(&self, session: &str, offers: Offers) {
        if let Some(found) = self.lock().sessions.get_mut(session) {
            found.offers = offers;
            found.last_seen = Instant::now();
        }
        self.notify();
    }

    /// The token a session registered with.
    pub fn token_of(&self, session: &str) -> Option<String> {
        self.lock()
            .sessions
            .get(session)
            .map(|found| found.token_id.clone())
    }

    /// A worker leaves: its task goes back in the queue.
    pub fn leave(&self, session: &str) {
        let mut inner = self.lock();
        remove_session(&mut inner, session, "it disconnected");
        drop(inner);
        self.notify();
    }

    /// Every worker registered with a revoked token leaves.
    pub fn revoke_token(&self, token_id: &str) {
        let mut inner = self.lock();
        let sessions = inner
            .sessions
            .iter()
            .filter(|(_, found)| found.token_id == token_id)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for session in sessions {
            remove_session(&mut inner, &session, "its token was revoked");
        }
        drop(inner);
        self.notify();
    }

    /// A ticket for a task about to be submitted, with a folder of its own
    /// in the exchange folder for the task's files. The ticket removes the
    /// folder when it is dropped.
    pub fn ticket(self: &Arc<Self>) -> Result<Ticket, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = self.exchange().join("tasks").join(&id);
        std::fs::create_dir_all(&dir).map_err(|error| {
            format!(
                "The worker exchange folder {} could not be written: {error}",
                dir.display()
            )
        })?;
        Ok(Ticket {
            board: Arc::clone(self),
            id,
            dir,
        })
    }

    /// Put `ticket`'s task on the board.
    fn submit(&self, ticket: &Ticket, task: NewTask) -> Result<(), String> {
        let (id, dir) = (ticket.id.clone(), ticket.dir.clone());
        let mut inputs = Vec::new();
        for (name, path) in task.inputs {
            let size = std::fs::metadata(&path)
                .map_err(|error| format!("Reading {}: {error}", path.display()))?
                .len();
            inputs.push((name, path, size));
        }
        let mut inner = self.lock();
        let seq = inner.next_seq;
        inner.next_seq += 1;
        inner.tasks.insert(
            id,
            Task {
                kind: task.kind,
                label: task.label,
                person: task.person,
                needs: task.needs,
                spec: task.spec,
                inputs,
                outputs: task.outputs,
                dir,
                seq,
                state: TaskState::Queued,
                attempts: 0,
                fraction: 0.0,
                unmatched_since: None,
            },
        );
        drop(inner);
        self.notify();
        Ok(())
    }

    /// Take a task off the board. A worker running it hears that the server
    /// no longer wants it at its next report.
    fn forget(&self, id: &str) {
        let mut inner = self.lock();
        if let Some(task) = inner.tasks.remove(id)
            && let TaskState::Leased { session, .. } | TaskState::Checking { session, .. } =
                &task.state
            && let Some(found) = inner.sessions.get_mut(session)
            && found.task.as_deref() == Some(id)
        {
            found.task = None;
            if found.local {
                inner.draining.insert(id.to_string());
            }
        }
        drop(inner);
        self.notify();
    }

    /// This server's worker is done with a task, however it ended.
    pub(super) fn local_done(&self, task: &str) {
        if self.lock().draining.remove(task) {
            self.notify();
        }
    }

    /// Wait, up to `limit`, until this server's worker has stopped a task
    /// taken off the board while it ran.
    fn wait_drained(&self, task: &str, limit: Duration) {
        let deadline = Instant::now() + limit;
        let mut inner = self.lock();
        while inner.draining.contains(task) {
            let now = Instant::now();
            if now >= deadline {
                return;
            }
            inner = self
                .changed
                .wait_timeout(inner, deadline - now)
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }

    /// Claim the first task `session` can run, without waiting.
    pub fn try_claim(&self, session: &str) -> Result<Option<Assignment>, Refusal> {
        let mut inner = self.lock();
        if inner.closed {
            return Ok(None);
        }
        let now = Instant::now();
        expire(&mut inner, now);
        let found = inner
            .sessions
            .get_mut(session)
            .ok_or(Refusal::UnknownSession)?;
        found.last_seen = now;
        if found.task.is_some() {
            // A worker runs one task at a time: a claim while it holds one
            // means it lost track of it, so the task goes back in the queue.
            let held = found.task.take().expect("the session holds a task");
            if let Some(task) = inner.tasks.get_mut(&held)
                && matches!(task.state, TaskState::Leased { .. })
            {
                task.state = TaskState::Queued;
                task.attempts += 1;
            }
        }
        let found = inner
            .sessions
            .get(session)
            .expect("the session is known")
            .clone();
        // This server's worker is the fallback: it takes a task only when no
        // connected worker of another machine can run it.
        let remote_offers = if found.local {
            inner
                .sessions
                .values()
                .filter(|other| !other.local && now.duration_since(other.last_seen) < ONLINE)
                .map(|other| other.offers.clone())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let next = inner
            .tasks
            .iter()
            .filter(|(_, task)| {
                matches!(task.state, TaskState::Queued)
                    && task.needs.met_by(&found.offers)
                    && !remote_offers.iter().any(|offers| task.needs.met_by(offers))
            })
            .min_by_key(|(_, task)| (!task.person, task.seq))
            .map(|(id, _)| id.clone());
        let Some(id) = next else {
            return Ok(None);
        };
        let lease = uuid::Uuid::new_v4().to_string();
        let task = inner.tasks.get_mut(&id).expect("the task is queued");
        task.state = TaskState::Leased {
            session: session.to_string(),
            lease: lease.clone(),
            expires: now + self.timings.lease,
        };
        task.fraction = 0.0;
        let shared_inputs = found.files.mode == FileMode::Shared;
        let assignment = Assignment {
            task: id.clone(),
            lease,
            kind: task.kind,
            label: task.label.clone(),
            spec: task.spec.clone(),
            inputs: task
                .inputs
                .iter()
                .map(|(name, path, size)| InputFile {
                    name: name.clone(),
                    size: *size,
                    path: shared_inputs.then(|| path.display().to_string()),
                })
                .collect(),
            outputs: task
                .outputs
                .iter()
                .map(|(name, required)| OutputFile {
                    name: name.clone(),
                    required: *required,
                    path: found
                        .exchange_shared
                        .then(|| task.dir.join(name).display().to_string()),
                })
                .collect(),
        };
        tracing::info!(
            task = id,
            worker = found.name,
            "A worker claimed {}",
            task.label
        );
        inner
            .sessions
            .get_mut(session)
            .expect("the session is known")
            .task = Some(id);
        drop(inner);
        self.notify();
        Ok(Some(assignment))
    }

    /// Claim a task for `session`, waiting up to `wait` for one. For the
    /// workers' routes.
    pub async fn claim(
        &self,
        session: &str,
        wait: Duration,
    ) -> Result<Option<Assignment>, Refusal> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let notified = self.claims.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if let Some(assignment) = self.try_claim(session)? {
                return Ok(Some(assignment));
            }
            if self.is_closed() {
                return Ok(None);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Ok(None);
            }
        }
    }

    /// Claim a task for `session`, waiting up to `wait` for one, on this
    /// thread. For this server's own worker.
    pub fn claim_blocking(
        &self,
        session: &str,
        wait: Duration,
    ) -> Result<Option<Assignment>, Refusal> {
        let deadline = Instant::now() + wait;
        loop {
            if let Some(assignment) = self.try_claim(session)? {
                return Ok(Some(assignment));
            }
            let inner = self.lock();
            if inner.closed {
                return Ok(None);
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            // Leases run out with no other change, so wake at least each
            // second to expire them.
            let wait = (deadline - now).min(Duration::from_secs(1));
            drop(self.changed.wait_timeout(inner, wait));
        }
    }

    /// Find the task `lease` is current for, as `session`'s.
    fn current<'a>(
        inner: &'a mut Inner,
        session: &str,
        task: &str,
        lease: &str,
    ) -> Result<&'a mut Task, Refusal> {
        if !inner.sessions.contains_key(session) {
            return Err(Refusal::UnknownSession);
        }
        let found = inner.tasks.get_mut(task).ok_or(Refusal::NotCurrent)?;
        match &found.state {
            TaskState::Leased {
                session: holder,
                lease: current,
                ..
            } if holder == session && current == lease => Ok(found),
            _ => Err(Refusal::NotCurrent),
        }
    }

    /// A worker reports how far it got, which renews its lease. Answers
    /// whether the server still wants the task.
    pub fn progress(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        fraction: f32,
    ) -> Result<bool, Refusal> {
        let mut inner = self.lock();
        let now = Instant::now();
        expire(&mut inner, now);
        if let Some(found) = inner.sessions.get_mut(session) {
            found.last_seen = now;
        }
        let found = match Self::current(&mut inner, session, task, lease) {
            Ok(found) => found,
            // The task is gone: the work that waited for it stopped.
            Err(Refusal::NotCurrent) => return Ok(true),
            Err(refusal) => return Err(refusal),
        };
        if let TaskState::Leased { expires, .. } = &mut found.state {
            *expires = now + self.timings.lease;
        }
        if fraction.is_finite() {
            found.fraction = fraction.clamp(0.0, 1.0);
        }
        drop(inner);
        self.notify();
        Ok(false)
    }

    /// Whether `lease` is still current for `session`'s task, read by this
    /// server's worker while it runs the task.
    pub fn still_wanted(&self, session: &str, task: &str, lease: &str) -> bool {
        let mut inner = self.lock();
        !inner.closed && Self::current(&mut inner, session, task, lease).is_ok()
    }

    /// Where an input is, for a worker to download it.
    pub fn input(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        name: &str,
    ) -> Result<PathBuf, Refusal> {
        let mut inner = self.lock();
        let found = Self::current(&mut inner, session, task, lease)?;
        found
            .inputs
            .iter()
            .find(|(input, _, _)| input == name)
            .map(|(_, path, _)| path.clone())
            .ok_or_else(|| Refusal::Invalid(format!("The task has no input named {name}")))
    }

    /// Where an uploaded output goes.
    pub fn output_path(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        name: &str,
    ) -> Result<PathBuf, Refusal> {
        let mut inner = self.lock();
        let found = Self::current(&mut inner, session, task, lease)?;
        if !found.outputs.iter().any(|(output, _)| output == name) {
            return Err(Refusal::Invalid(format!(
                "The task has no output named {name}"
            )));
        }
        Ok(found.dir.join(name))
    }

    /// A worker finished a task. Checks each output it lists against the
    /// file in the task folder, then hands the result to the waiting work.
    /// Reads every output, so callers run it off the async runtime.
    pub fn complete(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        completion: Completion,
    ) -> Result<(), Refusal> {
        let (dir, declared, worker) = {
            let mut inner = self.lock();
            let worker = inner
                .sessions
                .get(session)
                .map(|found| found.name.clone())
                .ok_or(Refusal::UnknownSession)?;
            let found = Self::current(&mut inner, session, task, lease)?;
            found.state = TaskState::Checking {
                session: session.to_string(),
            };
            (found.dir.clone(), found.outputs.clone(), worker)
        };
        let checked = check_outputs(&dir, &declared, &completion);
        let mut inner = self.lock();
        if let Some(found) = inner.sessions.get_mut(session) {
            found.task = None;
            found.last_seen = Instant::now();
        }
        let Some(found) = inner.tasks.get_mut(task) else {
            return Err(Refusal::NotCurrent);
        };
        let outcome = match checked {
            Ok(outputs) => Ok(Finished {
                result: completion.result,
                worker,
                outputs,
            }),
            Err(problem) => Err(format!(
                "{worker} sent back a result that failed its check: {problem}"
            )),
        };
        let refused = outcome.as_ref().err().cloned();
        found.state = TaskState::Finished(outcome);
        drop(inner);
        self.notify();
        match refused {
            Some(problem) => Err(Refusal::Invalid(problem)),
            None => Ok(()),
        }
    }

    /// A worker could not run a task: the waiting work fails with its
    /// error.
    pub fn fail(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        error: String,
    ) -> Result<(), Refusal> {
        let mut inner = self.lock();
        let worker = inner
            .sessions
            .get(session)
            .map(|found| found.name.clone())
            .unwrap_or_default();
        let found = Self::current(&mut inner, session, task, lease)?;
        found.state = TaskState::Finished(Err(format!("{error} (on {worker})")));
        if let Some(found) = inner.sessions.get_mut(session) {
            found.task = None;
            found.last_seen = Instant::now();
        }
        drop(inner);
        self.notify();
        Ok(())
    }

    /// The RC-Astro schemas each connected worker takes: the other
    /// machines' by name, then this server's, which runs a tool only when no
    /// other worker can.
    pub fn rc_astro_offers(&self) -> Vec<(String, bool, Vec<seiza_stacking::ExternalToolSchema>)> {
        let now = Instant::now();
        let inner = self.lock();
        let mut offers = inner
            .sessions
            .values()
            .filter(|found| now.duration_since(found.last_seen) < ONLINE)
            .filter(|found| !found.offers.rc_astro.is_empty())
            .map(|found| {
                (
                    found.name.clone(),
                    found.local,
                    found.offers.rc_astro.clone(),
                )
            })
            .collect::<Vec<_>>();
        offers.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        offers
    }

    /// The connected workers and the queued tasks, for Settings › Workers.
    pub fn summary(&self) -> (Vec<SessionSummary>, Vec<QueuedSummary>) {
        let now = Instant::now();
        let mut inner = self.lock();
        expire(&mut inner, now);
        inner
            .sessions
            .retain(|_, found| found.local || now.duration_since(found.last_seen) < FORGOTTEN);
        let mut sessions = inner
            .sessions
            .iter()
            .map(|(id, found)| SessionSummary {
                session: id.clone(),
                token_id: found.token_id.clone(),
                name: found.name.clone(),
                version: found.version.clone(),
                local: found.local,
                connected: now.duration_since(found.last_seen) < ONLINE,
                connected_at: found.connected_at,
                seconds_since_seen: now.duration_since(found.last_seen).as_secs(),
                threads: found.offers.threads,
                rc_astro: found
                    .offers
                    .rc_astro
                    .iter()
                    .filter(|schema| schema.licensed)
                    .map(|schema| format!("{} {}", schema.name, schema.cli_version))
                    .collect(),
                file_mode: found.files.mode,
                exchange_shared: found.exchange_shared,
                exchange_problem: found.exchange_problem.clone(),
                running: found
                    .task
                    .as_ref()
                    .and_then(|task| inner.tasks.get(task))
                    .map(|task| RunningSummary {
                        label: task.label.clone(),
                        fraction: task.fraction,
                    }),
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| {
            right
                .local
                .cmp(&left.local)
                .then_with(|| left.name.cmp(&right.name))
        });
        let mut queued = inner
            .tasks
            .values()
            .filter(|task| matches!(task.state, TaskState::Queued))
            .collect::<Vec<_>>();
        queued.sort_by_key(|task| (!task.person, task.seq));
        let queued = queued
            .into_iter()
            .map(|task| QueuedSummary {
                label: task.label.clone(),
                needs: task.needs.describe(),
            })
            .collect();
        (sessions, queued)
    }
}

/// Put leased tasks whose lease ran out back in the queue, or fail them
/// once they lost [`MAX_ATTEMPTS`] leases.
fn expire(inner: &mut Inner, now: Instant) {
    let mut lost = Vec::new();
    for (id, task) in &mut inner.tasks {
        if let TaskState::Leased {
            session, expires, ..
        } = &task.state
            && *expires <= now
            // This server's worker cannot stop answering while the server
            // runs, and a tool may report nothing for minutes.
            && !inner.sessions.get(session).is_some_and(|found| found.local)
        {
            lost.push((id.clone(), session.clone()));
        }
    }
    for (id, session) in lost {
        let worker = inner
            .sessions
            .get_mut(&session)
            .map(|found| {
                if found.task.as_deref() == Some(id.as_str()) {
                    found.task = None;
                }
                found.name.clone()
            })
            .unwrap_or_else(|| "The worker".into());
        let task = inner.tasks.get_mut(&id).expect("the task is leased");
        task.attempts += 1;
        tracing::warn!(
            task = id,
            worker,
            "A worker stopped answering about {}",
            task.label
        );
        task.state = if task.attempts >= MAX_ATTEMPTS {
            TaskState::Finished(Err(format!(
                "{worker} stopped answering, and the task had lost {MAX_ATTEMPTS} workers"
            )))
        } else {
            TaskState::Queued
        };
    }
}

/// Remove a session; the task it held goes back in the queue.
fn remove_session(inner: &mut Inner, session: &str, why: &str) {
    let Some(found) = inner.sessions.remove(session) else {
        return;
    };
    if let Some(probe) = &found.probe {
        let _ = std::fs::remove_file(&probe.path);
        let _ = std::fs::remove_file(&probe.reply_path);
    }
    tracing::info!(worker = found.name, "A worker left: {why}");
    if let Some(id) = found.task
        && let Some(task) = inner.tasks.get_mut(&id)
        && matches!(task.state, TaskState::Leased { .. })
    {
        task.attempts += 1;
        task.state = TaskState::Queued;
    }
}

/// Check every output a completion lists, and that each required output is
/// there. Returns each output's path.
fn check_outputs(
    dir: &Path,
    declared: &[(String, bool)],
    completion: &Completion,
) -> Result<BTreeMap<String, PathBuf>, String> {
    let mut outputs = BTreeMap::new();
    for written in &completion.outputs {
        if !declared.iter().any(|(name, _)| *name == written.name) {
            return Err(format!("it listed an output named {}", written.name));
        }
        let path = dir.join(&written.name);
        let (digest, size) = crate::remote_work::file_digest(&path)
            .map_err(|error| format!("{} is missing: {error}", written.name))?;
        if size != written.size || digest != written.sha256 {
            return Err(format!(
                "{} does not match the size and digest the worker sent",
                written.name
            ));
        }
        outputs.insert(written.name.clone(), path);
    }
    if let Some((name, _)) = declared
        .iter()
        .find(|(name, required)| *required && !outputs.contains_key(name))
    {
        return Err(format!("it did not write {name}"));
    }
    Ok(outputs)
}

/// A task submitted to the board. Waiting on it hands back what the worker
/// made; dropping it takes the task off the board and removes its files.
pub struct Ticket {
    board: Arc<Board>,
    id: String,
    dir: PathBuf,
}

/// How waiting on a ticket ended without a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitError {
    /// The work that waited was stopped.
    Stopped,
    Failed(String),
}

impl std::fmt::Display for WaitError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WaitError::Stopped => formatter.write_str("Stopped"),
            WaitError::Failed(message) => formatter.write_str(message),
        }
    }
}

impl Ticket {
    /// The task's own folder, where its inputs are written before it is
    /// submitted and its outputs arrive.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Put the task on the board, for the first worker that can run it.
    pub fn submit(&self, task: NewTask) -> Result<(), String> {
        self.board.submit(self, task)
    }

    /// Wait until a worker finishes the task, on this thread. `stopped` is
    /// read every half second; once it answers true the task leaves the
    /// board, and the worker running it stops at its next report.
    /// `progress` hears how much is done and which worker runs it: `None`
    /// for this server's own.
    pub fn wait(
        &self,
        stopped: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(f32, Option<&str>),
    ) -> Result<Finished, WaitError> {
        let mut reported: Option<(f32, Option<String>)> = None;
        let mut inner = self.board.lock();
        loop {
            let now = Instant::now();
            expire(&mut inner, now);
            if stopped() {
                drop(inner);
                self.board.forget(&self.id);
                // A tool this server runs is killed and its files removed
                // before the stop returns. A remote worker stops at its next
                // report, which the stop does not wait for.
                self.board.wait_drained(&self.id, DRAIN_LIMIT);
                return Err(WaitError::Stopped);
            }
            let online = inner
                .sessions
                .values()
                .filter(|found| now.duration_since(found.last_seen) < ONLINE)
                .map(|found| found.offers.clone())
                .collect::<Vec<_>>();
            let Some(task) = inner.tasks.get_mut(&self.id) else {
                return Err(WaitError::Failed("The task left the board".into()));
            };
            match &task.state {
                TaskState::Finished(_) => {
                    let TaskState::Finished(outcome) =
                        std::mem::replace(&mut task.state, TaskState::Queued)
                    else {
                        unreachable!("the task is finished");
                    };
                    inner.tasks.remove(&self.id);
                    return outcome.map_err(WaitError::Failed);
                }
                TaskState::Queued => {
                    if online.iter().any(|offers| task.needs.met_by(offers)) {
                        task.unmatched_since = None;
                    } else {
                        let since = *task.unmatched_since.get_or_insert(now);
                        if now.duration_since(since) >= self.board.timings.no_worker_grace {
                            let message =
                                format!("No connected worker offers {}", task.needs.describe());
                            inner.tasks.remove(&self.id);
                            return Err(WaitError::Failed(message));
                        }
                    }
                }
                TaskState::Leased { session, .. } | TaskState::Checking { session, .. } => {
                    let fraction = task.fraction;
                    let session = session.clone();
                    let worker = inner
                        .sessions
                        .get(&session)
                        .filter(|found| !found.local)
                        .map(|found| found.name.clone());
                    if reported
                        .as_ref()
                        .is_none_or(|(last, name)| *last != fraction || *name != worker)
                    {
                        drop(inner);
                        progress(fraction, worker.as_deref());
                        reported = Some((fraction, worker));
                        inner = self.board.lock();
                        continue;
                    }
                }
            }
            inner = self
                .board
                .changed
                .wait_timeout(inner, Duration::from_millis(500))
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .0;
        }
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.board.forget(&self.id);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests;
