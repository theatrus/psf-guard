//! The desktop app's controls for the worker it runs on this computer, in
//! the menu bar or the system tray: its settings, whether it starts at
//! login, and starting, pausing and stopping it. The worker is a separate
//! process (`psf-guard worker --tray`), so it keeps working while the app's
//! window is closed.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::commands::worker::control::{Connection, ControlClient, ControlFile};
use crate::commands::worker::{login, WorkerSettings, WorkerStatus};
use crate::remote_work::protocol::{FileMode, PathMap};

/// How long Start waits for the worker to answer, and Stop for it to go.
const START_LIMIT: Duration = Duration::from_secs(10);
const STOP_LIMIT: Duration = Duration::from_secs(40);

/// The worker's settings, without its token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkerSettingsView {
    pub server: String,
    pub name: Option<String>,
    pub files: FileMode,
    pub maps: Vec<PathMap>,
    pub work_dir: Option<String>,
    pub rc_astro: Option<String>,
    pub work: Vec<String>,
    pub slots: usize,
}

/// What Settings shows about this computer's worker.
#[derive(Debug, Clone, Serialize)]
pub struct DesktopWorker {
    /// Whether this system runs the worker in a menu bar or a tray.
    pub supported: bool,
    pub settings: Option<WorkerSettingsView>,
    /// Whether a token is saved; the token itself stays in the file.
    pub has_token: bool,
    pub start_at_login: bool,
    /// The running worker's status, or `None` when it is not running.
    pub status: Option<WorkerStatus>,
    pub settings_path: String,
}

/// The settings Save sends. A blank token keeps the saved one.
#[derive(Debug, Clone, Deserialize)]
pub struct SaveDesktopWorker {
    #[serde(flatten)]
    pub settings: WorkerSettingsView,
    #[serde(default)]
    pub token: Option<String>,
    pub start_at_login: bool,
}

fn settings_path() -> Result<PathBuf, String> {
    WorkerSettings::default_path().map_err(|error| error.to_string())
}

fn control_client() -> Result<Option<ControlClient>, String> {
    Ok(ControlClient::find(&ControlFile::path_for(
        &settings_path()?
    )))
}

async fn running_status() -> Result<Option<WorkerStatus>, String> {
    Ok(match control_client()? {
        Some(client) => client.status().await.ok(),
        None => None,
    })
}

async fn describe() -> Result<DesktopWorker, String> {
    let path = settings_path()?;
    let saved = WorkerSettings::load(&path).map_err(|error| format!("{error:#}"))?;
    Ok(DesktopWorker {
        supported: login::supported(),
        has_token: saved
            .as_ref()
            .is_some_and(|settings| !settings.token.trim().is_empty()),
        settings: saved.map(|settings| WorkerSettingsView {
            server: settings.server,
            name: settings.name,
            files: settings.files,
            maps: settings.maps,
            work_dir: settings.work_dir,
            rc_astro: settings.rc_astro,
            work: settings.work,
            slots: settings.slots,
        }),
        start_at_login: login::enabled(),
        status: running_status().await?,
        settings_path: path.display().to_string(),
    })
}

#[tauri::command]
pub async fn get_desktop_worker() -> Result<DesktopWorker, String> {
    describe().await
}

/// Save the worker's settings and whether it starts at login. A running
/// worker keeps its old settings until it is restarted.
#[tauri::command]
pub async fn save_desktop_worker(worker: SaveDesktopWorker) -> Result<DesktopWorker, String> {
    let path = settings_path()?;
    let token = match worker.token.filter(|token| !token.trim().is_empty()) {
        Some(token) => token.trim().to_string(),
        None => WorkerSettings::load(&path)
            .map_err(|error| format!("{error:#}"))?
            .map(|saved| saved.token)
            .unwrap_or_default(),
    };
    let view = worker.settings;
    let settings = WorkerSettings {
        server: view.server.trim().trim_end_matches('/').to_string(),
        token,
        name: view.name,
        files: view.files,
        maps: view.maps,
        work_dir: view.work_dir,
        rc_astro: view.rc_astro,
        work: view.work,
        slots: view.slots,
    };
    settings.save(&path).map_err(|error| format!("{error:#}"))?;
    if worker.start_at_login {
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        login::enable(&exe, &path).map_err(|error| format!("{error:#}"))?;
    } else {
        login::disable().map_err(|error| format!("{error:#}"))?;
    }
    describe().await
}

/// Quit a tray whose worker stopped, such as one the server refused, so a
/// new start replaces it.
async fn close_stopped_tray() -> Result<(), String> {
    let stopped = running_status().await?.is_some_and(|status| {
        matches!(status.connection, Connection::Failed | Connection::Stopped)
    });
    if stopped && let Some(client) = control_client()? {
        let _ = client.quit().await;
        let deadline = tokio::time::Instant::now() + STOP_LIMIT;
        while running_status().await?.is_some() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    Ok(())
}

/// Start the worker in the menu bar or the tray, unless it is running.
#[tauri::command]
pub async fn start_desktop_worker() -> Result<DesktopWorker, String> {
    close_stopped_tray().await?;
    if running_status().await?.is_none() {
        let path = settings_path()?;
        let settings = WorkerSettings::load(&path)
            .map_err(|error| format!("{error:#}"))?
            .ok_or("Save the worker's settings first")?;
        settings.validate().map_err(|error| format!("{error:#}"))?;
        let exe = std::env::current_exe().map_err(|error| error.to_string())?;
        std::process::Command::new(exe)
            .arg("worker")
            .arg("--tray")
            .arg("--config")
            .arg(&path)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|error| format!("The worker could not be started: {error}"))?;
        let deadline = tokio::time::Instant::now() + START_LIMIT;
        while running_status().await?.is_none() {
            if tokio::time::Instant::now() >= deadline {
                return Err("The worker did not start; click Open logs to see why".to_string());
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    describe().await
}

/// Stop the worker. Its running tasks stop, and the server queues them
/// again.
#[tauri::command]
pub async fn stop_desktop_worker() -> Result<DesktopWorker, String> {
    if let Some(client) = control_client()?
        && client.quit().await.is_ok()
    {
        let deadline = tokio::time::Instant::now() + STOP_LIMIT;
        while running_status().await?.is_some() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }
    describe().await
}

/// Pause the worker, so it takes no new tasks, or let it take them again.
#[tauri::command]
pub async fn pause_desktop_worker(paused: bool) -> Result<DesktopWorker, String> {
    let client = control_client()?.ok_or("The worker is not running")?;
    let paused = if paused {
        client.pause().await
    } else {
        client.resume().await
    };
    paused.map_err(|error| format!("The worker did not answer: {error}"))?;
    describe().await
}
