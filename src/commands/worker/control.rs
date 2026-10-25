//! What a running worker is doing, and the controls a desktop app or a
//! tray menu uses on it: pause, resume and quit.
//!
//! The tray worker (`psf-guard worker --tray`) serves them on a localhost
//! port until it exits. It writes the port and a secret it chose to a file
//! only its user can read ([`ControlFile`]); every request must send that
//! secret. The desktop app reads the file to find the worker.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde::{Deserialize, Serialize};

/// The header that carries the control secret.
pub const SECRET_HEADER: &str = "x-psf-worker-control";

/// Whether the worker is connected to its server.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Connection {
    /// The worker is starting, or trying to reach the server.
    #[default]
    Connecting,
    /// The server accepted the worker.
    Connected,
    /// The worker stopped with a problem: the server refused its token, or
    /// its settings cannot run.
    Failed,
    /// The worker has stopped.
    Stopped,
}

/// A task the worker is running.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunningTask {
    pub id: String,
    pub label: String,
    /// How much of the task is done, from 0 to 1.
    pub fraction: f32,
}

/// What a worker is doing, as its tray menu and the desktop app show it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WorkerStatus {
    pub server: String,
    pub name: String,
    /// The PSF Guard release the worker runs.
    pub version: String,
    pub connection: Connection,
    /// While paused, the worker takes no new tasks; running ones finish.
    pub paused: bool,
    pub tasks: Vec<RunningTask>,
    /// The latest problem, such as why the server could not be reached.
    /// Cleared when the worker connects.
    pub problem: Option<String>,
}

#[derive(Debug)]
struct Inner {
    status: Mutex<WorkerStatus>,
    paused: tokio::sync::watch::Sender<bool>,
    quit: tokio::sync::watch::Sender<bool>,
}

/// A running worker's status and controls. Clones share them.
#[derive(Debug, Clone)]
pub struct WorkerControl(Arc<Inner>);

impl Default for WorkerControl {
    fn default() -> Self {
        Self(Arc::new(Inner {
            status: Mutex::new(WorkerStatus {
                version: env!("CARGO_PKG_VERSION").into(),
                ..Default::default()
            }),
            paused: tokio::sync::watch::channel(false).0,
            quit: tokio::sync::watch::channel(false).0,
        }))
    }
}

impl WorkerControl {
    pub fn status(&self) -> WorkerStatus {
        self.lock().clone()
    }

    pub(super) fn update(&self, change: impl FnOnce(&mut WorkerStatus)) {
        change(&mut self.lock());
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, WorkerStatus> {
        self.0
            .status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Take no new tasks, or take them again.
    pub fn set_paused(&self, paused: bool) {
        self.0.paused.send_replace(paused);
        self.update(|status| status.paused = paused);
    }

    pub(super) fn paused(&self) -> tokio::sync::watch::Receiver<bool> {
        self.0.paused.subscribe()
    }

    /// Stop the worker: running tasks stop, and the server queues them
    /// again.
    pub fn quit(&self) {
        self.0.quit.send_replace(true);
    }

    /// Whether [`WorkerControl::quit`] was called.
    pub fn quit_requested(&self) -> bool {
        *self.0.quit.borrow()
    }

    /// Completes once [`WorkerControl::quit`] is called.
    pub async fn quitting(&self) {
        let mut quit = self.0.quit.subscribe();
        let _ = quit.wait_for(|quit| *quit).await;
    }

    /// Track a task while it runs; the task leaves the status when the
    /// returned guard drops.
    pub(super) fn task(&self, id: &str, label: &str) -> TaskEntry {
        self.update(|status| {
            status.tasks.push(RunningTask {
                id: id.into(),
                label: label.into(),
                fraction: 0.0,
            })
        });
        TaskEntry {
            control: self.clone(),
            id: id.into(),
        }
    }
}

/// A running task's place in the worker's status.
pub(super) struct TaskEntry {
    control: WorkerControl,
    id: String,
}

impl Drop for TaskEntry {
    fn drop(&mut self) {
        self.control
            .update(|status| status.tasks.retain(|task| task.id != self.id));
    }
}

/// Where the tray worker serves its controls, in a file beside its settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlFile {
    pub port: u16,
    pub secret: String,
    pub pid: u32,
}

impl ControlFile {
    /// The file beside a worker's settings file: `worker-control.json`
    /// for `worker.json`.
    pub fn path_for(settings: &Path) -> PathBuf {
        let stem = settings
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| "worker".into());
        settings.with_file_name(format!("{stem}-control.json"))
    }

    pub fn read(path: &Path) -> Result<Self> {
        let body = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        serde_json::from_slice(&body).with_context(|| format!("reading {}", path.display()))
    }

    fn write(&self, path: &Path) -> Result<()> {
        super::settings::write_private(path, &serde_json::to_vec_pretty(self)?)
    }
}

/// Serve `control` on a localhost port until `until` completes, after
/// writing where to reach it to `file`. The file is removed then.
pub async fn serve_control(
    control: WorkerControl,
    file: PathBuf,
    until: impl std::future::Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .context("opening the worker's control port")?;
    let secret = crate::remote_work::hex(&rand_bytes());
    ControlFile {
        port: listener.local_addr()?.port(),
        secret: secret.clone(),
        pid: std::process::id(),
    }
    .write(&file)?;
    let routes = axum::Router::new()
        .route("/status", get(status))
        .route("/pause", post(pause))
        .route("/resume", post(resume))
        .route("/quit", post(quit))
        .with_state(Served {
            control: control.clone(),
            secret: Arc::new(secret),
        });
    let served = axum::serve(listener, routes)
        .with_graceful_shutdown(until)
        .await;
    // Only this worker's file: a newer worker may have written its own.
    if ControlFile::read(&file).is_ok_and(|written| written.pid == std::process::id()) {
        let _ = std::fs::remove_file(&file);
    }
    served.context("serving the worker's controls")
}

/// What the control routes share.
#[derive(Clone)]
struct Served {
    control: WorkerControl,
    secret: Arc<String>,
}

impl Served {
    fn authorized(&self, headers: &axum::http::HeaderMap) -> bool {
        headers
            .get(SECRET_HEADER)
            .and_then(|value| value.to_str().ok())
            .is_some_and(|sent| constant_time_eq(sent.as_bytes(), self.secret.as_bytes()))
    }

    fn act(
        &self,
        headers: &axum::http::HeaderMap,
        action: impl FnOnce(&WorkerControl),
    ) -> StatusCode {
        if !self.authorized(headers) {
            return StatusCode::UNAUTHORIZED;
        }
        action(&self.control);
        StatusCode::NO_CONTENT
    }
}

async fn status(State(served): State<Served>, headers: axum::http::HeaderMap) -> Response {
    if !served.authorized(&headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    axum::Json(served.control.status()).into_response()
}

async fn pause(State(served): State<Served>, headers: axum::http::HeaderMap) -> StatusCode {
    served.act(&headers, |control| control.set_paused(true))
}

async fn resume(State(served): State<Served>, headers: axum::http::HeaderMap) -> StatusCode {
    served.act(&headers, |control| control.set_paused(false))
}

async fn quit(State(served): State<Served>, headers: axum::http::HeaderMap) -> StatusCode {
    served.act(&headers, WorkerControl::quit)
}

fn rand_bytes() -> [u8; 32] {
    let mut bytes = [0u8; 32];
    for chunk in bytes.chunks_mut(16) {
        chunk.copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    bytes
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// The desktop app's side: a worker's controls, found through its control
/// file.
pub struct ControlClient {
    http: reqwest::Client,
    base: String,
    secret: String,
}

impl ControlClient {
    /// The worker whose control file is at `path`, or `None` when no worker
    /// is running there.
    pub fn find(path: &Path) -> Option<Self> {
        let file = ControlFile::read(path).ok()?;
        Some(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                // A system proxy must not see the worker's secret.
                .no_proxy()
                .build()
                .ok()?,
            base: format!("http://127.0.0.1:{}", file.port),
            secret: file.secret,
        })
    }

    pub async fn status(&self) -> Result<WorkerStatus> {
        Ok(self
            .http
            .get(format!("{}/status", self.base))
            .header(SECRET_HEADER, &self.secret)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn post(&self, action: &str) -> Result<()> {
        self.http
            .post(format!("{}/{action}", self.base))
            .header(SECRET_HEADER, &self.secret)
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn pause(&self) -> Result<()> {
        self.post("pause").await
    }

    pub async fn resume(&self) -> Result<()> {
        self.post("resume").await
    }

    pub async fn quit(&self) -> Result<()> {
        self.post("quit").await
    }
}
