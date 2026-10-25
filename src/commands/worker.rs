//! `psf-guard worker`: run tasks for a PSF Guard server on this machine.
//!
//! The worker registers what it can run, claims tasks from the server one
//! at a time, and answers for each with its outputs. In shared mode it
//! reads a task's files where the server keeps them, through the maps it
//! was given; otherwise, or for a file it cannot see, the files go through
//! the server. See `docs/REMOTE_WORKERS.md`.

use std::collections::BTreeMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::StatusCode;
use tokio::io::AsyncWriteExt;

use crate::cli::WorkerFilesArg;
use crate::concurrency::Stop;
use crate::remote_work::protocol::{
    valid_file_name, Assignment, Completion, Failure, FileAccess, FileMode, PathMap, ProbeAnswer,
    ProbeResult, Progress, ProgressReply, Registered, Registration, WrittenOutput, DIGEST_HEADER,
    LEASE_HEADER, MAX_SLOTS, PROTOCOL_VERSION,
};
use crate::remote_work::{file_digest, map_path, LocalTools, WorkChoice};

/// The worker command's arguments, as the command line gives them.
pub struct WorkerArgs {
    pub server: String,
    pub token: Option<String>,
    pub name: Option<String>,
    pub files: WorkerFilesArg,
    pub maps: Vec<String>,
    pub work_dir: Option<String>,
    pub rc_astro: Option<String>,
    pub work: Vec<String>,
    pub slots: usize,
}

/// How a worker runs, once its arguments are read.
pub struct WorkerOptions {
    /// The server's address, without a trailing slash.
    pub server: String,
    pub token: String,
    pub name: String,
    pub files: FileAccess,
    pub work_dir: PathBuf,
    pub tools: LocalTools,
    /// The work it takes.
    pub work: WorkChoice,
    /// How many tasks it runs at once.
    pub slots: usize,
}

/// The longest wait between attempts to reach the server.
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long a stopped task waits for its tool to stop before its scratch
/// folder is removed.
const TOOL_STOP_LIMIT: Duration = Duration::from_secs(30);

pub fn run(args: WorkerArgs) -> Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info")),
        )
        .try_init();
    let options = options(args)?;
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(serve(options, async {
        let _ = tokio::signal::ctrl_c().await;
    }))
}

fn options(args: WorkerArgs) -> Result<WorkerOptions> {
    let token = args
        .token
        .or_else(|| std::env::var("PSF_GUARD_WORKER_TOKEN").ok())
        .filter(|token| !token.trim().is_empty())
        .context(
            "Give the worker token with --token or PSF_GUARD_WORKER_TOKEN; \
             create one in the server's Settings › Workers",
        )?;
    let maps = args
        .maps
        .iter()
        .map(|map| {
            let (server, local) = map
                .split_once('=')
                .with_context(|| format!("--map must be SERVER=LOCAL, not '{map}'"))?;
            anyhow::ensure!(
                !server.is_empty() && !local.is_empty(),
                "--map must be SERVER=LOCAL, not '{map}'"
            );
            Ok(PathMap {
                server: server.into(),
                local: local.into(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mode = if args.files == WorkerFilesArg::Shared || !maps.is_empty() {
        FileMode::Shared
    } else {
        FileMode::Remote
    };
    let rc_astro = crate::remote_work::rc_astro::locate(args.rc_astro.as_deref().map(Path::new));
    Ok(WorkerOptions {
        server: args.server.trim_end_matches('/').to_string(),
        token: token.trim().to_string(),
        name: args.name.unwrap_or_else(host_name),
        files: FileAccess { mode, maps },
        work_dir: args
            .work_dir
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("psf-guard-worker")),
        tools: LocalTools { rc_astro },
        work: WorkChoice::parse(&args.work).map_err(anyhow::Error::msg)?,
        slots: {
            anyhow::ensure!(
                (1..=MAX_SLOTS).contains(&args.slots),
                "--slots must be from 1 to {MAX_SLOTS}, not {}",
                args.slots
            );
            args.slots
        },
    })
}

/// This machine's host name, or "worker" when it cannot be read.
fn host_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "worker".into())
}

/// Why a request to the server failed.
#[derive(Debug)]
enum ClientError {
    /// The token is not valid: the worker cannot go on.
    Denied(String),
    /// The server does not know this session, such as after it restarted:
    /// the worker registers again.
    UnknownSession,
    /// The task was stopped, or went to another worker after this
    /// worker's lease ran out.
    NotCurrent,
    /// This worker is stopping.
    Stopping,
    /// The server refused the request, or could not be reached.
    Other(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Denied(message) | ClientError::Other(message) => {
                formatter.write_str(message)
            }
            ClientError::UnknownSession => formatter.write_str("The server forgot this worker"),
            ClientError::NotCurrent => formatter.write_str(
                "The task was stopped, or went to another worker after this worker's lease ran out",
            ),
            ClientError::Stopping => formatter.write_str("This worker is stopping"),
        }
    }
}

/// The server's workers API, as this worker's session sees it.
struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    fn new(server: &str, token: &str) -> Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .user_agent(concat!("psf-guard-worker/", env!("CARGO_PKG_VERSION")))
                .build()
                .context("building the worker's HTTP client")?,
            base: format!("{server}/api/workers/v1"),
            token: token.to_string(),
        })
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base))
            .bearer_auth(&self.token)
    }

    /// Send `request` and read the error message of a refusal.
    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ClientError> {
        let response = request.send().await.map_err(|error| {
            ClientError::Other(format!("The server could not be reached: {error}"))
        })?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        let message = response
            .json::<serde_json::Value>()
            .await
            .ok()
            .and_then(|body| body.get("error")?.as_str().map(str::to_string))
            .unwrap_or_else(|| format!("The server answered {status}"));
        Err(match status {
            StatusCode::UNAUTHORIZED => ClientError::Denied(message),
            StatusCode::NOT_FOUND => ClientError::UnknownSession,
            StatusCode::CONFLICT => ClientError::NotCurrent,
            _ => ClientError::Other(message),
        })
    }

    async fn json<T: serde::de::DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ClientError> {
        self.send(request).await?.json().await.map_err(|error| {
            ClientError::Other(format!("The server's answer could not be read: {error}"))
        })
    }

    async fn register(&self, registration: &Registration) -> Result<Registered, ClientError> {
        self.json(
            self.request(reqwest::Method::POST, "/register")
                .timeout(Duration::from_secs(60))
                .json(registration),
        )
        .await
    }

    async fn answer_probe(
        &self,
        session: &str,
        answer: &ProbeAnswer,
    ) -> Result<ProbeResult, ClientError> {
        self.json(
            self.request(reqwest::Method::POST, &format!("/sessions/{session}/probe"))
                .timeout(Duration::from_secs(60))
                .json(answer),
        )
        .await
    }

    async fn claim(
        &self,
        session: &str,
        poll: Duration,
    ) -> Result<Option<Assignment>, ClientError> {
        let response = self
            .send(
                self.request(reqwest::Method::POST, &format!("/sessions/{session}/claim"))
                    .timeout(poll + Duration::from_secs(30)),
            )
            .await?;
        if response.status() == StatusCode::NO_CONTENT {
            return Ok(None);
        }
        response
            .json()
            .await
            .map(Some)
            .map_err(|error| ClientError::Other(format!("The task could not be read: {error}")))
    }

    async fn leave(&self, session: &str) {
        let _ = self
            .send(
                self.request(reqwest::Method::POST, &format!("/sessions/{session}/leave"))
                    .timeout(Duration::from_secs(10)),
            )
            .await;
    }

    fn task(
        &self,
        method: reqwest::Method,
        session: &str,
        assignment: &Assignment,
        rest: &str,
    ) -> reqwest::RequestBuilder {
        self.request(
            method,
            &format!("/sessions/{session}/tasks/{}/{rest}", assignment.task),
        )
        .header(LEASE_HEADER, &assignment.lease)
    }

    async fn progress(
        &self,
        session: &str,
        assignment: &Assignment,
        fraction: f32,
    ) -> Result<bool, ClientError> {
        let reply: ProgressReply = self
            .json(
                self.task(reqwest::Method::POST, session, assignment, "progress")
                    .timeout(Duration::from_secs(30))
                    .json(&Progress { fraction }),
            )
            .await?;
        Ok(reply.stop)
    }

    async fn download(
        &self,
        session: &str,
        assignment: &Assignment,
        name: &str,
        to: &Path,
    ) -> Result<(), ClientError> {
        let mut response = self
            .send(self.task(
                reqwest::Method::GET,
                session,
                assignment,
                &format!("inputs/{name}"),
            ))
            .await?;
        let mut file = tokio::fs::File::create(to)
            .await
            .map_err(|error| ClientError::Other(format!("Writing {}: {error}", to.display())))?;
        while let Some(chunk) = response.chunk().await.map_err(|error| {
            ClientError::Other(format!("The download of {name} broke off: {error}"))
        })? {
            file.write_all(&chunk).await.map_err(|error| {
                ClientError::Other(format!("Writing {}: {error}", to.display()))
            })?;
        }
        file.flush()
            .await
            .map_err(|error| ClientError::Other(format!("Writing {}: {error}", to.display())))
    }

    async fn upload(
        &self,
        session: &str,
        assignment: &Assignment,
        name: &str,
        from: &Path,
        sha256: &str,
    ) -> Result<(), ClientError> {
        let file = tokio::fs::File::open(from)
            .await
            .map_err(|error| ClientError::Other(format!("Reading {}: {error}", from.display())))?;
        let length = file.metadata().await.map(|metadata| metadata.len()).ok();
        let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::with_capacity(
            file,
            crate::server::file_body::CHUNK,
        ));
        let mut request = self
            .task(
                reqwest::Method::PUT,
                session,
                assignment,
                &format!("outputs/{name}"),
            )
            .header(DIGEST_HEADER, sha256)
            .body(body);
        if let Some(length) = length {
            request = request.header(reqwest::header::CONTENT_LENGTH, length);
        }
        self.send(request).await.map(drop)
    }

    async fn complete(
        &self,
        session: &str,
        assignment: &Assignment,
        completion: &Completion,
    ) -> Result<(), ClientError> {
        self.send(
            self.task(reqwest::Method::POST, session, assignment, "complete")
                .json(completion),
        )
        .await
        .map(drop)
    }

    async fn fail(
        &self,
        session: &str,
        assignment: &Assignment,
        error: String,
    ) -> Result<(), ClientError> {
        self.send(
            self.task(reqwest::Method::POST, session, assignment, "fail")
                .timeout(Duration::from_secs(30))
                .json(&Failure { error }),
        )
        .await
        .map(drop)
    }
}

/// What every slot of a joined worker shares: the client, the options and
/// the session.
struct Joined {
    client: Client,
    options: WorkerOptions,
    session: String,
    poll: Duration,
    heartbeat: Duration,
}

/// How a slot stopped claiming tasks.
enum SlotEnd {
    /// The worker is stopping.
    Stopped,
    /// The server forgot the session: the worker registers again.
    Rejoin,
    /// The server refused the token.
    Denied(String),
}

/// Run tasks for the server until `shutdown` completes, up to
/// `options.slots` at once. Returns an error only when the server refuses
/// the token.
pub async fn serve(options: WorkerOptions, shutdown: impl Future<Output = ()>) -> Result<()> {
    let client = Client::new(&options.server, &options.token)?;
    tokio::pin!(shutdown);
    let (tools, work) = (options.tools.clone(), options.work.clone());
    let (mut offers, problem) = tokio::task::spawn_blocking(move || {
        tools.offers(crate::concurrency::logical_cores(), &work)
    })
    .await?;
    offers.slots = options.slots;
    if offers.rc_astro.is_empty() {
        match (&options.tools.rc_astro, problem) {
            (None, _) => tracing::warn!(
                "rc-astro is not installed here; this worker has no work to accept yet"
            ),
            (Some(_), Some(problem)) => tracing::warn!(
                "rc-astro could not be read ({problem}); this worker has no work to accept yet"
            ),
            (Some(_), None) => tracing::warn!(
                "--work leaves out every RC-Astro tool; this worker has no work to accept yet"
            ),
        }
    } else {
        for schema in &offers.rc_astro {
            tracing::info!(
                "Accepts {} {}{}",
                schema.name,
                schema.cli_version,
                if schema.licensed {
                    ""
                } else {
                    " (not licensed)"
                }
            );
        }
    }
    let registration = Registration {
        protocol: PROTOCOL_VERSION,
        name: options.name.clone(),
        version: env!("CARGO_PKG_VERSION").into(),
        offers,
        files: options.files.clone(),
    };
    let mut options = Some(options);
    let mut client = Some(client);
    let mut backoff = Duration::from_secs(1);
    loop {
        let registering = client.as_ref().expect("the client is idle");
        let registered = tokio::select! {
            () = &mut shutdown => return Ok(()),
            registered = registering.register(&registration) => registered,
        };
        let registered = match registered {
            Ok(registered) => registered,
            Err(ClientError::Denied(message)) => anyhow::bail!(message),
            Err(error) => {
                tracing::warn!("{error}; trying again in {}s", backoff.as_secs());
                tokio::select! {
                    () = &mut shutdown => return Ok(()),
                    () = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        };
        backoff = Duration::from_secs(1);
        let settings = options.as_ref().expect("the options are idle");
        tracing::info!(
            "Joined {} as {}, running up to {} task(s) at once",
            settings.server,
            registration.name,
            settings.slots
        );
        if let Some(probe) = &registered.probe {
            let answer = answer_probe(&settings.files.maps, &probe.path, &probe.reply_path).await;
            match registering.answer_probe(&registered.session, &answer).await {
                Ok(ProbeResult { shared: true, .. }) => tracing::info!(
                    "Shared files: this worker reads and writes task files in the server's exchange folder"
                ),
                Ok(ProbeResult { reason, .. }) => tracing::warn!(
                    "Shared files are off: {}. Task files go through the server.",
                    reason.unwrap_or_else(|| "the server's exchange folder is not visible here".into())
                ),
                Err(error) => tracing::warn!("The shared-files check failed: {error}"),
            }
        }
        let joined = Arc::new(Joined {
            client: client.take().expect("the client is idle"),
            options: options.take().expect("the options are idle"),
            session: registered.session,
            poll: Duration::from_secs(registered.poll_seconds),
            heartbeat: Duration::from_secs((registered.lease_seconds / 6).max(1)),
        });
        let (stopping, stop_receiver) = tokio::sync::watch::channel(false);
        let mut slots = tokio::task::JoinSet::new();
        for _ in 0..joined.options.slots {
            slots.spawn(run_slot(Arc::clone(&joined), stop_receiver.clone()));
        }
        let mut shutting_down = false;
        let mut ended = None;
        loop {
            tokio::select! {
                () = &mut shutdown, if !shutting_down => {
                    tracing::info!("Stopping: running tasks stop, and the server queues them again");
                    shutting_down = true;
                    stopping.send_replace(true);
                }
                slot = slots.join_next() => match slot {
                    None => break,
                    Some(Ok(SlotEnd::Stopped)) => {}
                    Some(Ok(end)) => {
                        stopping.send_replace(true);
                        if !matches!(ended, Some(SlotEnd::Denied(_))) {
                            ended = Some(end);
                        }
                    }
                    Some(Err(error)) => tracing::error!("A worker slot stopped unexpectedly: {error}"),
                },
            }
        }
        let joined = Arc::into_inner(joined).expect("every slot has ended");
        if shutting_down {
            joined.client.leave(&joined.session).await;
            return Ok(());
        }
        match ended {
            Some(SlotEnd::Denied(message)) => anyhow::bail!(message),
            _ => tracing::warn!("The server forgot this worker; joining again"),
        }
        client = Some(joined.client);
        options = Some(joined.options);
    }
}

/// One slot: claim a task, run it, and claim the next, until the worker
/// stops or the server forgets the session.
async fn run_slot(
    joined: Arc<Joined>,
    mut stopping: tokio::sync::watch::Receiver<bool>,
) -> SlotEnd {
    let mut backoff = Duration::from_secs(1);
    loop {
        if *stopping.borrow() {
            return SlotEnd::Stopped;
        }
        let claimed = tokio::select! {
            _ = stopping.changed() => return SlotEnd::Stopped,
            claimed = joined.client.claim(&joined.session, joined.poll) => claimed,
        };
        match claimed {
            Ok(None) => {}
            Ok(Some(assignment)) => match run_task(&joined, &assignment, stopping.clone()).await {
                Ok(()) => {}
                Err(ClientError::Stopping) => return SlotEnd::Stopped,
                Err(ClientError::UnknownSession) => return SlotEnd::Rejoin,
                Err(ClientError::Denied(message)) => return SlotEnd::Denied(message),
                Err(_) => {}
            },
            Err(ClientError::Denied(message)) => return SlotEnd::Denied(message),
            Err(ClientError::UnknownSession) => return SlotEnd::Rejoin,
            Err(error) => {
                tracing::warn!("{error}; trying again in {}s", backoff.as_secs());
                tokio::select! {
                    _ = stopping.changed() => return SlotEnd::Stopped,
                    () = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(MAX_BACKOFF);
                continue;
            }
        }
        backoff = Duration::from_secs(1);
    }
}

/// Read the probe through the maps, and write the same bytes to the reply.
async fn answer_probe(maps: &[PathMap], probe: &str, reply: &str) -> ProbeAnswer {
    let (probe, reply) = (map_path(maps, probe), map_path(maps, reply));
    tokio::task::spawn_blocking(move || {
        let (Some(probe), Some(reply)) = (probe, reply) else {
            return ProbeAnswer {
                digest: None,
                problem: Some("no --map reaches the server's exchange folder".into()),
            };
        };
        let content = match std::fs::read(&probe) {
            Ok(content) => content,
            Err(error) => {
                return ProbeAnswer {
                    digest: None,
                    problem: Some(format!(
                        "{} could not be read here: {error}",
                        probe.display()
                    )),
                }
            }
        };
        if let Err(error) = std::fs::write(&reply, &content) {
            return ProbeAnswer {
                digest: None,
                problem: Some(format!(
                    "{} could not be written here: {error}",
                    reply.display()
                )),
            };
        }
        use sha2::{Digest, Sha256};
        ProbeAnswer {
            digest: Some(crate::remote_work::hex(&Sha256::digest(&content))),
            problem: None,
        }
    })
    .await
    .unwrap_or_else(|error| ProbeAnswer {
        digest: None,
        problem: Some(error.to_string()),
    })
}

/// Refuse a task whose id or file names could reach outside this worker's
/// folder: the id must be a task id the server minted, and each name a
/// plain file name.
fn check_assignment(assignment: &Assignment) -> Result<(), String> {
    let minted =
        uuid::Uuid::parse_str(&assignment.task).is_ok_and(|id| id.to_string() == assignment.task);
    if !minted {
        return Err(format!("The task id {:?} is not valid", assignment.task));
    }
    let names = assignment
        .inputs
        .iter()
        .map(|input| &input.name)
        .chain(assignment.outputs.iter().map(|output| &output.name));
    for name in names {
        if !valid_file_name(name) {
            return Err(format!("The task file name {name:?} is not valid"));
        }
    }
    Ok(())
}

/// Run one task and answer for it. Returns how the task ended when the
/// slot cannot go on: the worker is stopping, or the server forgot the
/// session or refused the token. Other problems are logged and reported.
async fn run_task(
    joined: &Joined,
    assignment: &Assignment,
    stopping: tokio::sync::watch::Receiver<bool>,
) -> Result<(), ClientError> {
    let (client, session) = (&joined.client, joined.session.as_str());
    if let Err(problem) = check_assignment(assignment) {
        tracing::warn!("Refused a task: {problem}");
        let _ = client.fail(session, assignment, problem).await;
        return Ok(());
    }
    tracing::info!(task = assignment.task, "Running {}", assignment.label);
    let scratch = joined.options.work_dir.join(&assignment.task);
    let outcome = run_task_in(joined, assignment, stopping, &scratch).await;
    let _ = tokio::fs::remove_dir_all(&scratch).await;
    match outcome {
        Ok(()) => {
            tracing::info!(task = assignment.task, "Finished {}", assignment.label);
            Ok(())
        }
        Err(ClientError::NotCurrent) => {
            tracing::info!(
                task = assignment.task,
                "Dropped {}: it was stopped, or went to another worker",
                assignment.label
            );
            Ok(())
        }
        Err(
            ended @ (ClientError::Stopping | ClientError::UnknownSession | ClientError::Denied(_)),
        ) => Err(ended),
        Err(error) => {
            tracing::warn!(
                task = assignment.task,
                "{} failed: {error}",
                assignment.label
            );
            let _ = client.fail(session, assignment, error.to_string()).await;
            Ok(())
        }
    }
}

/// Reports progress while a task's work runs, which renews its lease, and
/// stops the work when the server or the worker says to.
struct Heartbeat<'a> {
    joined: &'a Joined,
    assignment: &'a Assignment,
    beat: tokio::time::Interval,
    fraction: tokio::sync::watch::Receiver<f32>,
    stopping: tokio::sync::watch::Receiver<bool>,
}

impl Heartbeat<'_> {
    /// Run `work` while reporting progress. Returns the reason the work
    /// must end early, dropping `work`, when the task was stopped or went to
    /// another worker, the server forgot the session or refused the token,
    /// or the worker is stopping.
    async fn during<T>(
        &mut self,
        work: impl Future<Output = Result<T, ClientError>>,
    ) -> Result<T, ClientError> {
        tokio::pin!(work);
        loop {
            if *self.stopping.borrow() {
                return Err(ClientError::Stopping);
            }
            tokio::select! {
                done = &mut work => return done,
                _ = self.stopping.changed() => return Err(ClientError::Stopping),
                _ = self.beat.tick() => {
                    let done = *self.fraction.borrow();
                    match self
                        .joined
                        .client
                        .progress(&self.joined.session, self.assignment, done)
                        .await
                    {
                        Ok(false) => {}
                        Ok(true) => return Err(ClientError::NotCurrent),
                        Err(error @ (ClientError::NotCurrent
                            | ClientError::UnknownSession
                            | ClientError::Denied(_))) => return Err(error),
                        Err(error) => tracing::warn!("A progress report failed: {error}"),
                    }
                }
            }
        }
    }
}

async fn run_task_in(
    joined: &Joined,
    assignment: &Assignment,
    stopping: tokio::sync::watch::Receiver<bool>,
    scratch: &Path,
) -> Result<(), ClientError> {
    let (client, session, options) = (&joined.client, joined.session.as_str(), &joined.options);
    let local_error = |error: std::io::Error| ClientError::Other(error.to_string());
    let (fraction_sender, fraction) = tokio::sync::watch::channel(0.0f32);
    let mut heartbeat = Heartbeat {
        joined,
        assignment,
        beat: tokio::time::interval(joined.heartbeat),
        fraction,
        stopping,
    };
    tokio::fs::create_dir_all(scratch)
        .await
        .map_err(local_error)?;
    let maps = &options.files.maps;
    let mut inputs = BTreeMap::new();
    for input in &assignment.inputs {
        let shared = input
            .path
            .as_deref()
            .and_then(|path| map_path(maps, path))
            .filter(|path| {
                std::fs::metadata(path).is_ok_and(|metadata| metadata.len() == input.size)
            });
        let path = match shared {
            Some(path) => path,
            None => {
                let to = scratch.join(&input.name);
                heartbeat
                    .during(client.download(session, assignment, &input.name, &to))
                    .await?;
                to
            }
        };
        inputs.insert(input.name.clone(), path);
    }
    // Outputs go straight into the lease's folder on the server when it is
    // visible here, and are uploaded otherwise.
    let shared_output_dir = assignment
        .outputs
        .iter()
        .find_map(|output| output.path.as_deref())
        .and_then(|path| map_path(maps, path))
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .filter(|dir| dir.is_dir());
    let output_dir = shared_output_dir
        .clone()
        .unwrap_or_else(|| scratch.join("out"));
    tokio::fs::create_dir_all(&output_dir)
        .await
        .map_err(local_error)?;

    let stop = Stop::new();
    let mut executed = {
        let (kind, spec, tools, output_dir, stop) = (
            assignment.kind,
            assignment.spec.clone(),
            options.tools.clone(),
            output_dir.clone(),
            stop.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let cancel = seiza_stacking::CancelSignal::new(move || stop.requested());
            crate::remote_work::execute(
                kind,
                spec,
                &inputs,
                &output_dir,
                &tools,
                &cancel,
                &mut |done| {
                    fraction_sender.send_replace(done);
                },
            )
        })
    };
    let ran = heartbeat
        .during(async {
            (&mut executed)
                .await
                .map_err(|error| {
                    ClientError::Other(format!("The task stopped unexpectedly: {error}"))
                })?
                .map_err(ClientError::Other)
        })
        .await;
    let result = match ran {
        Ok(result) => result,
        Err(ended) => {
            // The tool stops at its next check of the stop; wait for it, so
            // nothing writes to the scratch folder once it is removed.
            stop.request();
            let _ = tokio::time::timeout(TOOL_STOP_LIMIT, &mut executed).await;
            return Err(ended);
        }
    };
    let mut written = Vec::new();
    for output in &assignment.outputs {
        let path = output_dir.join(&output.name);
        if !path.exists() {
            continue;
        }
        let digested = path.clone();
        let (sha256, size) = heartbeat
            .during(async {
                tokio::task::spawn_blocking(move || file_digest(&digested))
                    .await
                    .map_err(|error| ClientError::Other(error.to_string()))?
                    .map_err(local_error)
            })
            .await?;
        if shared_output_dir.is_none() {
            heartbeat
                .during(client.upload(session, assignment, &output.name, &path, &sha256))
                .await?;
        }
        written.push(WrittenOutput {
            name: output.name.clone(),
            size,
            sha256,
        });
    }
    // Not under the heartbeat: the server checks the outputs before it
    // answers, and keeps the task while it does.
    client
        .complete(
            session,
            assignment,
            &Completion {
                result,
                outputs: written,
            },
        )
        .await
}

#[cfg(test)]
mod tests;
