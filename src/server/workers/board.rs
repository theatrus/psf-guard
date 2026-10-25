//! The task board: tasks waiting for a worker, the workers connected to
//! this server, and the lease each running task is under.
//!
//! Work on this server submits a task and waits on its [`Ticket`]. A worker
//! claims the first queued task it can run and holds a lease on it. Each
//! progress report renews the lease. A task goes back in the queue when its
//! lease runs out or its worker leaves, and fails once that has happened
//! [`MAX_ATTEMPTS`] times. The server checks each output's size and digest
//! before the waiting work reads it. A late answer under an old lease is
//! refused.
//!
//! Each kind of task has a [`WorkPlacement`]: workers first with this
//! server's own worker as the fallback, workers only, or this server only.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

pub use crate::db_registry::WorkPlacement;
use crate::remote_work::protocol::{
    Assignment, Completion, ExchangeProbe, FileAccess, FileMode, InputFile, Offers, OutputFile,
    ProbeAnswer, ProbeResult, Progress, ProgressReply, Registration, TaskKind, MAX_SLOTS,
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
/// The most sessions one token may hold; registering another removes the
/// one heard from longest ago.
const SESSIONS_PER_TOKEN: usize = 16;
/// How many times a task may go back in the queue before it fails.
pub const MAX_ATTEMPTS: u32 = 3;
/// How long a queued task waits when no connected worker can run it, for
/// one to connect, before it fails.
const NO_WORKER_GRACE: Duration = Duration::from_secs(30);
/// How long a stop waits for this server's worker to stop the task.
const DRAIN_LIMIT: Duration = Duration::from_secs(30);
/// The folder PSF Guard keeps task files in, inside the exchange folder.
/// Only this folder's contents are ever removed.
pub const EXCHANGE_SUBFOLDER: &str = "psf-guard-exchange";
/// Task files older than this are left over from a server that stopped
/// without removing them. A server start removes them; younger ones may
/// belong to another server sharing the folder.
const STALE_TASK_FILES: Duration = Duration::from_secs(6 * 3600);

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
    /// Stacking at this Seiza stacking output version, on this PSF Guard
    /// release.
    Stack {
        stacking_version: String,
        release: String,
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
            TaskNeeds::Stack {
                stacking_version,
                release,
            } => {
                offers.work.contains(&TaskKind::StackChannel)
                    && offers.stacking_version.as_deref() == Some(stacking_version)
                    && offers.stacking_release.as_deref() == Some(release)
            }
        }
    }

    /// What the task needs, as a person reads it.
    fn describe(&self) -> String {
        match self {
            TaskNeeds::RcAstro {
                name, cli_version, ..
            } => format!("{name} {cli_version}"),
            TaskNeeds::Stack {
                stacking_version,
                release,
            } => format!("stacking with PSF Guard {release} and Seiza stacking {stacking_version}"),
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
    /// The tasks it holds leases on, at most its slots.
    tasks: Vec<String>,
    /// This server's own worker.
    local: bool,
}

impl Session {
    /// This server's worker counts as connected while the server runs: a
    /// tool it runs may report nothing for minutes.
    fn online(&self, now: Instant) -> bool {
        self.local || now.duration_since(self.last_seen) < ONLINE
    }

    fn slots(&self) -> usize {
        self.offers.slots.clamp(1, MAX_SLOTS)
    }

    fn release(&mut self, task: &str) {
        self.tasks.retain(|held| held != task);
    }

    /// The worker, across its sessions: a worker that registers again keeps
    /// what it failed.
    fn token_and_name(&self) -> String {
        format!("{}/{}", self.token_id, self.name)
    }
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
    /// Workers of other machines that failed it: this server runs it
    /// instead, when it can.
    refused_by: HashSet<String>,
    /// Events the worker reported that the waiting work has not read yet.
    events: Vec<serde_json::Value>,
    /// How many events the worker reported, all told.
    events_received: u64,
    /// The waiting work asked the worker to stop and finish with what it
    /// has.
    stop_requested: bool,
    /// Stops the work when this server's worker runs it.
    local_stop: Option<crate::concurrency::Stop>,
    /// When the worker last reported or sent an output: a stop that lets
    /// the worker finish waits while it keeps doing so.
    last_report: Option<Instant>,
}

impl Task {
    /// Where the worker holding `lease` writes the outputs. Each lease has
    /// its own folder, so a worker that lost its lease cannot overwrite the
    /// next worker's outputs.
    fn lease_dir(&self, lease: &str) -> PathBuf {
        self.dir.join(lease)
    }

    /// Put the task back in the queue, or fail it once it went back
    /// [`MAX_ATTEMPTS`] times. `why` finishes "the task went back in the
    /// queue because …". Returns the old lease's folder, for the caller to
    /// remove once it has released the board's lock.
    #[must_use]
    fn requeue(&mut self, why: String) -> Option<PathBuf> {
        let old = match &self.state {
            TaskState::Leased { lease, .. } => Some(self.lease_dir(lease)),
            _ => None,
        };
        self.attempts += 1;
        // The next worker reports its events from the start, as the
        // work's first events again.
        self.events.clear();
        self.events_received = 0;
        self.last_report = None;
        tracing::warn!("{} went back in the queue: {why}", self.label);
        self.state = if self.attempts >= MAX_ATTEMPTS {
            TaskState::Finished(Err(format!(
                "The task went back in the queue {MAX_ATTEMPTS} times; the last time {why}"
            )))
        } else {
            TaskState::Queued
        };
        old
    }
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
            Refusal::NotCurrent => formatter.write_str(
                "The task was stopped, or went to another worker after this worker's lease ran out",
            ),
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
    draining: HashSet<String>,
    placements: HashMap<TaskKind, WorkPlacement>,
    /// Lease folders to remove once the lock is released.
    trash: Vec<PathBuf>,
    next_seq: u64,
    closed: bool,
}

impl Inner {
    fn placement(&self, kind: TaskKind) -> WorkPlacement {
        self.placements
            .get(&kind)
            .copied()
            .unwrap_or_else(|| WorkPlacement::default_for(kind))
    }

    /// Whether `session` may run `task`: it offers what the task needs, the
    /// task's placement allows it, and, for this server's worker under
    /// workers first, no connected worker of another machine can run it.
    fn may_run(&self, session: &Session, task: &Task, now: Instant) -> bool {
        let placement = self.placement(task.kind);
        if !self.may_run_queued(session, task) {
            return false;
        }
        !(session.local
            && placement == WorkPlacement::WorkersFirst
            && self
                .sessions
                .values()
                .any(|other| !other.local && other.online(now) && self.may_run_queued(other, task)))
    }
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
    /// How many tasks it runs at once.
    pub slots: usize,
    /// The licensed RC-Astro tools it takes, as "StarXTerminator 2.6.6".
    pub rc_astro: Vec<String>,
    /// The Seiza stacking version it stacks builds with, when it takes
    /// stack builds.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stacking_version: Option<String>,
    pub file_mode: FileMode,
    /// Whether task files reach it by path.
    pub exchange_shared: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exchange_problem: Option<String>,
    /// The tasks it runs now, with how much of each is done.
    pub running: Vec<RunningSummary>,
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

    /// Remove the lease folders requeued tasks left, outside the lock: the
    /// exchange folder may be on a slow share.
    fn sweep(&self) {
        let trash = std::mem::take(&mut self.lock().trash);
        for folder in trash {
            let _ = std::fs::remove_dir_all(folder);
        }
    }

    /// Expire leases that ran out, and wake whoever waits when one did.
    fn expire(&self, inner: &mut Inner, now: Instant) {
        if expire(inner, now) {
            self.notify();
        }
    }

    /// The exchange folder chosen in Settings, or the default.
    pub fn exchange(&self) -> PathBuf {
        self.exchange
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// The folder inside the exchange folder that task files go in.
    fn files(&self) -> PathBuf {
        self.exchange().join(EXCHANGE_SUBFOLDER)
    }

    /// Move the exchange folder. Tasks already submitted keep theirs.
    pub fn set_exchange(&self, folder: PathBuf) {
        *self
            .exchange
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = folder;
    }

    /// Remove task folders and probes a stopped server left in the exchange
    /// folder. Only entries named by a task or session id and older than
    /// six hours go: another server may share the folder.
    pub fn remove_stale_files(&self) {
        let files = self.files();
        let now = SystemTime::now();
        for (folder, directories) in [("tasks", true), ("probes", false)] {
            let Ok(entries) = std::fs::read_dir(files.join(folder)) else {
                continue;
            };
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let id = name.split('.').next().unwrap_or_default();
                let stale = entry
                    .metadata()
                    .and_then(|metadata| metadata.modified())
                    .ok()
                    .and_then(|modified| now.duration_since(modified).ok())
                    .is_some_and(|age| age > STALE_TASK_FILES);
                if uuid::Uuid::parse_str(id).is_err() || !stale {
                    continue;
                }
                let _ = if directories {
                    std::fs::remove_dir_all(entry.path())
                } else {
                    std::fs::remove_file(entry.path())
                };
            }
        }
    }

    /// Whether a connected worker of another machine can run a task with
    /// `needs`.
    pub fn remote_can_run(&self, needs: &TaskNeeds) -> bool {
        let now = Instant::now();
        self.lock()
            .sessions
            .values()
            .any(|found| !found.local && found.online(now) && needs.met_by(&found.offers))
    }

    /// Where a kind of work runs.
    pub fn set_placement(&self, kind: TaskKind, placement: WorkPlacement) {
        self.lock().placements.insert(kind, placement);
        self.notify();
    }

    pub fn placement(&self, kind: TaskKind) -> WorkPlacement {
        self.lock().placement(kind)
    }

    /// Stop handing out work: claims answer at once, and this server's
    /// worker stops what it runs and ends.
    pub fn close(&self) {
        let mut inner = self.lock();
        inner.closed = true;
        for stop in inner
            .tasks
            .values()
            .filter_map(|task| task.local_stop.as_ref())
        {
            stop.request();
        }
        drop(inner);
        self.notify();
    }

    /// The stop this server's worker runs a task under: requested when the
    /// task leaves the board or the waiting work stops it.
    pub(super) fn attach_stop(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        stop: crate::concurrency::Stop,
    ) -> Result<(), Refusal> {
        let mut inner = self.lock();
        let found = Self::current(&mut inner, session, task, lease)?;
        if found.stop_requested {
            stop.request();
        }
        found.local_stop = Some(stop);
        Ok(())
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
        let now = Instant::now();
        let mut inner = self.lock();
        forget_quiet_sessions(&mut inner, now);
        // A worker that registers again under the same name, such as after a
        // restart, replaces its old session: its tasks go back in the queue.
        let mut replaced = inner
            .sessions
            .iter()
            .filter(|(_, old)| old.token_id == token_id && old.name == registration.name)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        // One token holds a limited number of sessions.
        let mut of_token = inner
            .sessions
            .iter()
            .filter(|(id, old)| old.token_id == token_id && !replaced.contains(id))
            .map(|(id, old)| (old.last_seen, id.clone()))
            .collect::<Vec<_>>();
        of_token.sort();
        let excess = (of_token.len() + 1).saturating_sub(SESSIONS_PER_TOKEN);
        replaced.extend(of_token.into_iter().take(excess).map(|(_, id)| id));
        for old in replaced {
            remove_session(&mut inner, &old, "connected again");
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
                last_seen: now,
                tasks: Vec::new(),
                local,
            },
        );
        drop(inner);
        self.notify();
        self.sweep();
        session
    }

    fn write_probe(&self, session: &str) -> std::io::Result<Probe> {
        let folder = self.files().join("probes");
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

    /// A worker leaves: its tasks go back in the queue.
    pub fn leave(&self, session: &str) {
        let mut inner = self.lock();
        remove_session(&mut inner, session, "disconnected");
        drop(inner);
        self.notify();
        self.sweep();
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
            remove_session(&mut inner, &session, "had its token revoked");
        }
        drop(inner);
        self.notify();
        self.sweep();
    }

    /// A ticket for a task about to be submitted, with a folder of its own
    /// in the exchange folder for the task's files. The ticket removes the
    /// folder when it is dropped.
    pub fn ticket(self: &Arc<Self>) -> Result<Ticket, String> {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = self.files().join("tasks").join(&id);
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
                refused_by: HashSet::new(),
                events: Vec::new(),
                events_received: 0,
                stop_requested: false,
                local_stop: None,
                last_report: None,
            },
        );
        drop(inner);
        self.notify();
        Ok(())
    }

    /// Take a task off the board. The worker running it learns at its next
    /// report that the task was stopped.
    fn forget(&self, id: &str) {
        let mut inner = self.lock();
        if let Some(stop) = inner
            .tasks
            .get(id)
            .and_then(|task| task.local_stop.as_ref())
        {
            stop.request();
        }
        if let Some(task) = inner.tasks.remove(id)
            && let TaskState::Leased { session, .. } | TaskState::Checking { session, .. } =
                &task.state
            && let Some(found) = inner.sessions.get_mut(session)
            && found.tasks.iter().any(|held| held == id)
        {
            found.release(id);
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

    /// Claim the first task `session` may run, without waiting. A worker
    /// with every slot busy claims nothing.
    pub fn try_claim(&self, session: &str) -> Result<Option<Assignment>, Refusal> {
        let mut inner = self.lock();
        if inner.closed {
            return Ok(None);
        }
        let now = Instant::now();
        self.expire(&mut inner, now);
        let found = inner
            .sessions
            .get_mut(session)
            .ok_or(Refusal::UnknownSession)?;
        found.last_seen = now;
        if found.tasks.len() >= found.slots() {
            return Ok(None);
        }
        let found = found.clone();
        let next = inner
            .tasks
            .iter()
            .filter(|(_, task)| {
                // A task its waiting work stopped goes to no new worker.
                matches!(task.state, TaskState::Queued)
                    && !task.stop_requested
                    && inner.may_run(&found, task, now)
            })
            .min_by_key(|(_, task)| (!task.person, task.seq))
            .map(|(id, _)| id.clone());
        let Some(id) = next else {
            return Ok(None);
        };
        let lease = uuid::Uuid::new_v4().to_string();
        let task = inner.tasks.get_mut(&id).expect("the task is queued");
        let lease_dir = task.lease_dir(&lease);
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
                        .then(|| lease_dir.join(name).display().to_string()),
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
            .tasks
            .push(id);
        drop(inner);
        self.notify();
        self.sweep();
        // Outside the lock: the exchange folder may be on a slow share. The
        // worker cannot write there before it has the assignment.
        if let Err(error) = std::fs::create_dir_all(&lease_dir) {
            tracing::warn!("{} could not be created: {error}", lease_dir.display());
        }
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
            // Leases run out with no other change, so look again at least
            // each second.
            let next = (tokio::time::Instant::now() + Duration::from_secs(1)).min(deadline);
            if tokio::time::timeout_at(next, notified).await.is_err()
                && tokio::time::Instant::now() >= deadline
            {
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
    /// whether the worker should stop: the task was stopped, or went to
    /// another worker.
    pub fn progress(
        &self,
        session: &str,
        task: &str,
        lease: &str,
        progress: Progress,
    ) -> Result<ProgressReply, Refusal> {
        let mut inner = self.lock();
        let now = Instant::now();
        self.expire(&mut inner, now);
        if let Some(found) = inner.sessions.get_mut(session) {
            found.last_seen = now;
        }
        let found = match Self::current(&mut inner, session, task, lease) {
            Ok(found) => found,
            Err(Refusal::NotCurrent) => {
                return Ok(ProgressReply {
                    stop: true,
                    finish: false,
                    events_received: 0,
                })
            }
            Err(refusal) => return Err(refusal),
        };
        if let TaskState::Leased { expires, .. } = &mut found.state {
            *expires = now + self.timings.lease;
        }
        found.last_report = Some(now);
        if progress.fraction.is_finite() {
            found.fraction = progress.fraction.clamp(0.0, 1.0);
        }
        // Events already received, from a report sent again, are skipped.
        let skip = found.events_received.saturating_sub(progress.first_event) as usize;
        if progress.first_event <= found.events_received {
            let fresh = progress.events.into_iter().skip(skip).collect::<Vec<_>>();
            found.events_received += fresh.len() as u64;
            found.events.extend(fresh);
        }
        let reply = ProgressReply {
            stop: false,
            finish: found.stop_requested,
            events_received: found.events_received,
        };
        drop(inner);
        self.notify();
        self.sweep();
        Ok(reply)
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
            .ok_or_else(|| Refusal::Invalid(format!("The task has no input called {name}")))
    }

    /// Where an uploaded output goes: the lease's own folder.
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
                "The task has no output called {name}"
            )));
        }
        found.last_report = Some(Instant::now());
        Ok(found.lease_dir(lease).join(name))
    }

    /// A worker finished a task. Checks each output it lists against the
    /// file in the lease's folder, then hands the result to the waiting
    /// work. Reads every output, so callers run it off the async runtime.
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
            (found.lease_dir(lease), found.outputs.clone(), worker)
        };
        let checked = check_outputs(&dir, &declared, &completion);
        let mut inner = self.lock();
        if let Some(found) = inner.sessions.get_mut(session) {
            found.release(task);
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
        let refuser = inner.sessions.get(session).cloned();
        let found = Self::current(&mut inner, session, task, lease)?;
        let (needs, kind) = (found.needs.clone(), found.kind);
        // A worker of another machine that fails the task, such as one whose
        // GPU driver or licence broke, hands it to this server when this
        // server can run it.
        let fallback = refuser.as_ref().is_some_and(|refuser| !refuser.local)
            && inner.placement(kind).allows(true)
            && inner
                .sessions
                .values()
                .any(|other| other.local && needs.met_by(&other.offers));
        let found = inner.tasks.get_mut(task).expect("the task is current");
        if let (true, Some(refuser)) = (fallback, &refuser) {
            found.refused_by.insert(refuser.token_and_name());
            let old = found.requeue(format!("{worker} could not run it: {error}"));
            inner.trash.extend(old);
        } else {
            found.state = TaskState::Finished(Err(format!("{error} (on {worker})")));
        }
        if let Some(found) = inner.sessions.get_mut(session) {
            found.release(task);
            found.last_seen = Instant::now();
        }
        drop(inner);
        self.notify();
        self.sweep();
        Ok(())
    }

    /// The RC-Astro schemas of each connected worker that may run RC-Astro
    /// steps under their placement: other machines' by name, then this
    /// server's, which is the fallback.
    pub fn rc_astro_offers(&self) -> Vec<(String, bool, Vec<seiza_stacking::ExternalToolSchema>)> {
        let now = Instant::now();
        let inner = self.lock();
        let placement = inner.placement(TaskKind::RcAstroStep);
        let mut offers = inner
            .sessions
            .values()
            .filter(|found| found.online(now) && placement.allows(found.local))
            .filter(|found| {
                found.offers.work.contains(&TaskKind::RcAstroStep)
                    && !found.offers.rc_astro.is_empty()
            })
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
    /// Forgets workers not heard from for an hour.
    pub fn summary(&self) -> (Vec<SessionSummary>, Vec<QueuedSummary>) {
        let now = Instant::now();
        self.sweep();
        let mut inner = self.lock();
        self.expire(&mut inner, now);
        forget_quiet_sessions(&mut inner, now);
        let mut sessions = inner
            .sessions
            .iter()
            .map(|(id, found)| SessionSummary {
                session: id.clone(),
                token_id: found.token_id.clone(),
                name: found.name.clone(),
                version: found.version.clone(),
                local: found.local,
                connected: found.online(now),
                connected_at: found.connected_at,
                seconds_since_seen: now.duration_since(found.last_seen).as_secs(),
                threads: found.offers.threads,
                slots: found.slots(),
                rc_astro: found
                    .offers
                    .rc_astro
                    .iter()
                    .filter(|_| found.offers.work.contains(&TaskKind::RcAstroStep))
                    .filter(|schema| schema.licensed)
                    .map(|schema| format!("{} {}", schema.name, schema.cli_version))
                    .collect(),
                stacking_version: found
                    .offers
                    .work
                    .contains(&TaskKind::StackChannel)
                    .then(|| found.offers.stacking_version.clone())
                    .flatten(),
                file_mode: found.files.mode,
                exchange_shared: found.exchange_shared,
                exchange_problem: found.exchange_problem.clone(),
                running: found
                    .tasks
                    .iter()
                    .filter_map(|task| inner.tasks.get(task))
                    .map(|task| RunningSummary {
                        label: task.label.clone(),
                        fraction: task.fraction,
                    })
                    .collect(),
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

/// Put leased tasks whose lease ran out back in the queue. Returns whether
/// any did.
fn expire(inner: &mut Inner, now: Instant) -> bool {
    let mut lost = Vec::new();
    for (id, task) in &inner.tasks {
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
    let expired = !lost.is_empty();
    for (id, session) in lost {
        let worker = inner
            .sessions
            .get_mut(&session)
            .map(|found| {
                found.release(&id);
                found.name.clone()
            })
            .unwrap_or_else(|| "the worker".into());
        let task = inner.tasks.get_mut(&id).expect("the task is leased");
        let old = task.requeue(format!("{worker} stopped answering"));
        inner.trash.extend(old);
    }
    expired
}

/// Remove sessions not heard from for an hour, other than this server's.
fn forget_quiet_sessions(inner: &mut Inner, now: Instant) {
    let quiet = inner
        .sessions
        .iter()
        .filter(|(_, found)| !found.local && now.duration_since(found.last_seen) >= FORGOTTEN)
        .map(|(id, _)| id.clone())
        .collect::<Vec<_>>();
    for session in quiet {
        remove_session(inner, &session, "stopped answering");
    }
}

/// Remove a session and its probe files; its tasks go back in the queue.
/// `why` finishes "the worker …".
fn remove_session(inner: &mut Inner, session: &str, why: &str) {
    let Some(found) = inner.sessions.remove(session) else {
        return;
    };
    if let Some(probe) = &found.probe {
        let _ = std::fs::remove_file(&probe.path);
        let _ = std::fs::remove_file(&probe.reply_path);
    }
    tracing::info!(worker = found.name, "A worker left: it {why}");
    for id in &found.tasks {
        if let Some(task) = inner.tasks.get_mut(id)
            && matches!(task.state, TaskState::Leased { .. })
        {
            let old = task.requeue(format!("{} {why}", found.name));
            inner.trash.extend(old);
        }
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
            return Err(format!("it listed an output called {}", written.name));
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

/// A task submitted to the board. Waiting on it returns what the worker
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
    /// submitted. Each lease's outputs go in a folder inside it.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Put the task on the board, for the first worker that can run it.
    pub fn submit(&self, task: NewTask) -> Result<(), String> {
        self.board.submit(self, task)
    }

    /// Wait until a worker finishes the task, on this thread. `stopped` is
    /// read every half second; once it returns true the task leaves the
    /// board, and the worker running it stops at its next report.
    /// `progress` is told how much is done and which worker runs it: `None`
    /// for this server's own.
    pub fn wait(
        &self,
        stopped: &dyn Fn() -> bool,
        progress: &mut dyn FnMut(f32, Option<&str>),
    ) -> Result<Finished, WaitError> {
        self.wait_with(Waiting {
            stopped,
            progress,
            events: &mut |_| {},
            finish_on_stop: None,
        })
    }

    /// [`Ticket::wait`], with the events the worker reports, and, with
    /// `finish_on_stop`, a stop that lets the worker finish with what it
    /// has.
    pub fn wait_with(&self, waiting: Waiting<'_>) -> Result<Finished, WaitError> {
        let Waiting {
            stopped,
            progress,
            events,
            finish_on_stop,
        } = waiting;
        let mut reported: Option<(f32, Option<String>)> = None;
        let mut stop_deadline: Option<Instant> = None;
        let mut inner = self.board.lock();
        loop {
            let now = Instant::now();
            self.board.expire(&mut inner, now);
            let running = inner.tasks.get(&self.id).is_some_and(|task| {
                matches!(
                    task.state,
                    TaskState::Leased { .. } | TaskState::Checking { .. }
                )
            });
            let finishing = match (finish_on_stop, stop_deadline) {
                // The worker has until the limit after its last report or
                // upload, so a slow upload of what it has still arrives.
                (Some(limit), Some(deadline)) => {
                    let reported = inner
                        .tasks
                        .get(&self.id)
                        .and_then(|task| task.last_report)
                        .map(|last| last + limit);
                    now < reported.map_or(deadline, |reported| reported.max(deadline))
                }
                (None, Some(deadline)) => now < deadline,
                (Some(limit), None) if running && stopped() => {
                    // The worker's next report is answered with a request
                    // to finish with what it has.
                    let task = inner.tasks.get_mut(&self.id).expect("the task runs");
                    task.stop_requested = true;
                    if let Some(stop) = &task.local_stop {
                        stop.request();
                    }
                    stop_deadline = Some(now + limit);
                    self.board.notify();
                    true
                }
                _ => false,
            };
            if !finishing && (stop_deadline.is_some() || stopped()) {
                drop(inner);
                self.board.forget(&self.id);
                // A tool this server runs is killed and its files removed
                // before the stop returns. A remote worker stops at its next
                // report, which the stop does not wait for.
                self.board.wait_drained(&self.id, DRAIN_LIMIT);
                return Err(WaitError::Stopped);
            }
            let Some(task) = inner.tasks.get_mut(&self.id) else {
                return Err(WaitError::Failed("The task left the board".into()));
            };
            if !task.events.is_empty() {
                let received = std::mem::take(&mut task.events);
                drop(inner);
                for event in received {
                    events(event);
                }
                inner = self.board.lock();
                continue;
            }
            let task = inner.tasks.get(&self.id).expect("the task is on the board");
            match &task.state {
                TaskState::Finished(_) => {
                    let task = inner.tasks.remove(&self.id).expect("the task is finished");
                    let TaskState::Finished(outcome) = task.state else {
                        unreachable!("the task is finished");
                    };
                    return outcome.map_err(WaitError::Failed);
                }
                TaskState::Queued => {
                    let runnable = inner
                        .sessions
                        .values()
                        .any(|found| found.online(now) && inner.may_run_queued(found, task));
                    let placement = inner.placement(task.kind);
                    let task = inner.tasks.get_mut(&self.id).expect("the task is queued");
                    if runnable {
                        task.unmatched_since = None;
                    } else {
                        let since = *task.unmatched_since.get_or_insert(now);
                        if now.duration_since(since) >= self.board.timings.no_worker_grace {
                            let message = format!(
                                "No connected worker offers {}{}",
                                task.needs.describe(),
                                match placement {
                                    WorkPlacement::WorkersOnly =>
                                        ", and Settings › Workers keeps this work off this server",
                                    WorkPlacement::ThisServer =>
                                        "; Settings › Workers keeps this work on this server",
                                    WorkPlacement::WorkersFirst => "",
                                }
                            );
                            inner.tasks.remove(&self.id);
                            return Err(WaitError::Failed(message));
                        }
                    }
                }
                TaskState::Leased { session, .. } | TaskState::Checking { session, .. } => {
                    let fraction = task.fraction;
                    let worker = inner
                        .sessions
                        .get(session)
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

impl Inner {
    /// Whether `session` could run a queued task, ignoring the fallback
    /// rule: this server's worker counts too, since it takes over once the
    /// other workers are gone.
    fn may_run_queued(&self, session: &Session, task: &Task) -> bool {
        task.needs.met_by(&session.offers)
            && self.placement(task.kind).allows(session.local)
            && !task.refused_by.contains(&session.token_and_name())
    }
}

/// How [`Ticket::wait_with`] waits.
pub struct Waiting<'a> {
    /// Read every half second.
    pub stopped: &'a dyn Fn() -> bool,
    /// Told how much is done and which worker runs it: `None` for this
    /// server's own.
    pub progress: &'a mut dyn FnMut(f32, Option<&str>),
    /// Told each event the worker reports, in order.
    pub events: &'a mut dyn FnMut(serde_json::Value),
    /// On a stop, how long a running task has to finish with what it has
    /// before it leaves the board. `None` takes it off the board at once.
    pub finish_on_stop: Option<Duration>,
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.board.forget(&self.id);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[cfg(test)]
mod tests;
