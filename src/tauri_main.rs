use crate::cli::PregenerationConfig;
use crate::db_registry::{DbEntry, DbRegistry};
use crate::desktop_server::Settings as DesktopSettings;
use crate::server::shutdown::Shutdown;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, Layer};

/// Tauri-side server bootstrap parameters. Built once at startup; rebuilt
/// (with the latest registry contents) on `restart_server`.
#[derive(Debug, Clone)]
struct TauriServerConfig {
    static_dir: Option<String>,
    pregeneration: PregenerationConfig,
    storage: crate::server::storage::StorageConfig,
}

#[derive(Clone)]
struct ServerState {
    status: Arc<Mutex<DesktopServerStatus>>,
    /// Keep the binding while a fixed-port server drains and restarts, so
    /// another process cannot take the port between server instances.
    listener: Arc<Mutex<Option<std::net::TcpListener>>>,
    /// Serialize settings saves and restarts; two restarts must never launch
    /// servers which write the same job journal at once.
    lifecycle: Arc<tokio::sync::Mutex<()>>,
    /// The registry file. Commands read it from disk each time: Settings
    /// save through the local server, so no in-memory copy stays current.
    registry_path: Arc<Mutex<PathBuf>>,
    server_shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
    /// The running server, which ends once its work has drained and it has
    /// written the job journal for the last time.
    server_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
    /// The running server's stop, which says what it still waits for.
    server_stop: Arc<Mutex<Option<Arc<Shutdown>>>>,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct DesktopServerStatus {
    server_port: Option<u16>,
    active_server_port: Option<u16>,
    server_url: Option<String>,
    starting: bool,
    error: Option<String>,
}

struct ReadyListener {
    listener: std::net::TcpListener,
    ready: oneshot::Sender<()>,
}

/// How long a restart waits for the old server's work. Past this, a job is
/// most likely stuck, as on a share that stopped answering.
const RESTART_WAIT: Duration = Duration::from_secs(120);

impl ServerState {
    fn note_error(&self, error: &str) {
        let mut status = self.status.lock().unwrap();
        status.server_url = None;
        status.starting = false;
        status.error = Some(error.to_string());
        self.listener.lock().unwrap().take();
    }

    fn reserved_port(&self) -> Option<u16> {
        self.listener
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|listener| listener.local_addr().ok())
            .map(|address| address.port())
    }

    async fn save_port(&self, port: Option<u16>) -> Result<DesktopServerStatus, String> {
        let _guard = self.lifecycle.lock().await;
        let settings = DesktopSettings { server_port: port };
        settings.validate().map_err(|e| e.to_string())?;
        // A refused save leaves both the saved preference and running server intact.
        if port.is_some() && port != self.reserved_port() {
            settings.bind().map_err(|e| format!("{e:#}"))?;
        }
        settings
            .save(&DesktopSettings::path(&registry_path(self)?))
            .map_err(|e| format!("Could not save desktop settings: {e:#}"))?;
        self.status.lock().unwrap().server_port = port;
        Ok(self.status.lock().unwrap().clone())
    }

    async fn start_server(
        &self,
        listener: std::net::TcpListener,
        registry: DbRegistry,
        base: TauriServerConfig,
    ) -> Result<String, String> {
        let port = listener.local_addr().map_err(|e| e.to_string())?.port();
        let server_listener = listener.try_clone().map_err(|e| e.to_string())?;
        *self.listener.lock().unwrap() = Some(listener);
        let url = format!("http://127.0.0.1:{port}");
        {
            let mut status = self.status.lock().unwrap();
            status.server_url = None;
            status.starting = true;
            status.error = None;
        }
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let (ready_tx, ready_rx) = oneshot::channel();
        let stop = Shutdown::new();
        *self.server_shutdown.lock().unwrap() = Some(shutdown_tx);
        *self.server_stop.lock().unwrap() = Some(Arc::clone(&stop));
        let state = self.clone();
        let registry_path = registry_path(self)?;
        let task = tokio::spawn(async move {
            if let Err(error) = start_server_for_tauri(
                ReadyListener {
                    listener: server_listener,
                    ready: ready_tx,
                },
                registry.databases,
                registry.astrometry,
                base,
                registry_path,
                shutdown_rx,
                stop,
            )
            .await
            {
                let message = format!("Could not start the desktop server: {error:#}");
                tracing::error!("{message}");
                state.note_error(&message);
            } else {
                state.status.lock().unwrap().server_url = None;
            }
        });
        *self.server_task.lock().unwrap() = Some(task);
        if ready_rx.await.is_err() {
            // The ready sender is dropped as initialization unwinds. Wait for
            // the task to record the actual error before reporting it to UI.
            let task = self.server_task.lock().unwrap().take();
            if let Some(task) = task
                && let Err(error) = task.await
            {
                self.note_error(&format!(
                    "The desktop server stopped during startup: {error}"
                ));
            }
            let error = self
                .status
                .lock()
                .unwrap()
                .error
                .clone()
                .unwrap_or_else(|| "The desktop server stopped before it was ready.".to_string());
            self.note_error(&error);
            return Err(error);
        }
        let mut status = self.status.lock().unwrap();
        if let Some(error) = &status.error {
            return Err(error.clone());
        }
        status.server_url = Some(url.clone());
        status.active_server_port = status.server_port;
        status.starting = false;
        Ok(url)
    }

    async fn restart(&self, base: TauriServerConfig) -> Result<String, String> {
        let _guard = self.lifecycle.lock().await;
        let registry = load_registry(self)?;
        let settings = DesktopSettings::load(&DesktopSettings::path(&registry_path(self)?))
            .map_err(|e| format!("{e:#}"))?;
        // Reuse the reserved socket at the same fixed port. Reserve a
        // different port before stopping the working server.
        let retained = if settings.server_port.is_some() {
            self.listener
                .lock()
                .unwrap()
                .as_ref()
                .filter(|listener| {
                    listener.local_addr().ok().map(|address| address.port()) == settings.server_port
                })
                .map(std::net::TcpListener::try_clone)
                .transpose()
                .map_err(|e| format!("Could not retain the desktop server socket: {e}"))?
        } else {
            None
        };
        let listener = match retained {
            Some(listener) => listener,
            None => settings.bind().map_err(|e| format!("{e:#}"))?,
        };
        self.stop_server(RESTART_WAIT).await?;
        self.status.lock().unwrap().server_port = settings.server_port;
        self.start_server(listener, registry, base).await
    }

    /// Stop the running server and wait, up to `wait`, until it has ended.
    /// A stop lets work in hand reach a safe point, then writes the job
    /// journal; the next server reads that file when it starts and writes
    /// it from then on, so it must not start before the last write. When
    /// the wait runs out, the error says what the old server still waits
    /// for, and the server is left to finish: the next restart waits for it
    /// again, and restarting the app ends it.
    async fn stop_server(&self, wait: Duration) -> Result<(), String> {
        let shutdown = self.server_shutdown.lock().unwrap().take();
        if let Some(shutdown_tx) = shutdown
            && shutdown_tx.send(()).is_err()
        {
            tracing::warn!("Failed to send shutdown signal");
        }
        let task = self.server_task.lock().unwrap().take();
        let Some(mut task) = task else {
            return Ok(());
        };
        tracing::info!("🛑 Waiting for the server to finish its work before restarting");
        match tokio::time::timeout(wait, &mut task).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(error)) => {
                tracing::warn!("The server ended badly: {error}");
                Ok(())
            }
            Err(_) => {
                let waiting = self
                    .server_stop
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|stop| stop.running())
                    .unwrap_or_default();
                *self.server_task.lock().unwrap() = Some(task);
                let message = format!(
                    "The server is still finishing its work after {} s{}; restart the app \
                     to end it",
                    wait.as_secs(),
                    if waiting.is_empty() {
                        String::new()
                    } else {
                        format!(" ({})", waiting.join("; "))
                    }
                );
                tracing::warn!("{message}");
                Err(message)
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn main() {
    init_logging();

    let registry_path = DbRegistry::default_path().expect("Could not resolve config path");
    let initial_registry = DbRegistry::load_or_init(&registry_path).unwrap_or_else(|err| {
        eprintln!(
            "Warning: failed to load config at {}: {} — starting with empty registry",
            registry_path.display(),
            err
        );
        DbRegistry::default()
    });

    let server_config = TauriServerConfig {
        static_dir: None,
        pregeneration: PregenerationConfig::default(),
        storage: crate::server::storage::StorageConfig::defaulting_to(
            dirs::cache_dir()
                .unwrap_or_else(|| std::env::temp_dir().join("psf-guard-cache"))
                .join("psf-guard"),
        ),
    };

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let state = ServerState {
        status: Arc::new(Mutex::new(DesktopServerStatus {
            starting: true,
            ..Default::default()
        })),
        listener: Arc::new(Mutex::new(None)),
        lifecycle: Arc::new(tokio::sync::Mutex::new(())),
        registry_path: Arc::new(Mutex::new(registry_path.clone())),
        server_shutdown: Arc::new(Mutex::new(None)),
        server_task: Arc::new(Mutex::new(None)),
        server_stop: Arc::new(Mutex::new(None)),
    };
    let settings = DesktopSettings::load(&DesktopSettings::path(&registry_path));
    match settings.and_then(|settings| {
        state.status.lock().unwrap().server_port = settings.server_port;
        settings.bind()
    }) {
        Ok(listener) => {
            let state = state.clone();
            let base = server_config.clone();
            rt.spawn(async move {
                let _guard = state.lifecycle.lock().await;
                if let Err(error) = state.start_server(listener, initial_registry, base).await {
                    state.note_error(&error);
                }
            });
        }
        Err(error) => state.note_error(&format!("{error:#}")),
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .manage(server_config)
        .invoke_handler(tauri::generate_handler![
            get_server_url,
            get_desktop_server_settings,
            set_desktop_server_port,
            pick_database_file,
            pick_folder,
            get_default_nina_database_path,
            get_current_configuration,
            show_image_in_folder,
            restart_application,
            restart_server,
            is_configuration_valid,
            get_log_folder,
            open_log_folder
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Log to the console, as the server does, and to the daily file in the
/// app's log folder: a window has no console on Windows or macOS. `RUST_LOG`
/// sets both levels; info by default. Without a writable folder the app
/// still starts, logging to the console alone.
fn init_logging() {
    let filter = || {
        tracing_subscriber::filter::EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info"))
    };
    let console = tracing_subscriber::fmt::layer()
        .with_target(false)
        .with_level(true)
        .with_thread_ids(false)
        .with_filter(filter());
    let dir = psf_guard_log_dir();
    let appender = dir
        .as_deref()
        .map(crate::desktop_log::file_appender)
        .transpose();
    match appender {
        Ok(appender) => {
            let file = appender.map(|appender| {
                tracing_subscriber::fmt::layer()
                    .with_ansi(false)
                    .with_target(false)
                    .with_writer(appender)
                    .with_filter(filter())
            });
            tracing_subscriber::registry()
                .with(console)
                .with(file)
                .init();
            if let Some(dir) = dir {
                tracing::info!("logging to {}", dir.display());
            }
        }
        Err(error) => {
            tracing_subscriber::registry().with(console).init();
            tracing::warn!("no log file: {error}");
        }
    }
}

fn psf_guard_log_dir() -> Option<PathBuf> {
    crate::desktop_log::log_dir()
}

/// The folder the log files are written to.
#[tauri::command]
fn get_log_folder() -> Option<String> {
    psf_guard_log_dir().map(|dir| dir.display().to_string())
}

/// Open the log folder in the file manager.
#[tauri::command]
async fn open_log_folder() -> Result<(), String> {
    let dir = psf_guard_log_dir().ok_or_else(|| "This system has no log folder".to_string())?;
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("Could not create {}: {e}", dir.display()))?;
        let status = folder_command(&dir)?
            .status()
            .map_err(|e| format!("Could not open the file manager: {e}"))?;
        if status.success() || cfg!(target_os = "windows") {
            // Explorer exits 1 even when it opened the folder.
            Ok(())
        } else {
            Err(format!("The file manager exited with status {status}"))
        }
    })
    .await
    .map_err(|e| format!("File manager task failed: {e}"))?
}

#[cfg(target_os = "macos")]
fn folder_command(dir: &Path) -> Result<Command, String> {
    let mut command = Command::new("open");
    command.arg(dir);
    Ok(command)
}

#[cfg(target_os = "windows")]
fn folder_command(dir: &Path) -> Result<Command, String> {
    let mut command = Command::new("explorer.exe");
    command.arg(dir);
    Ok(command)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn folder_command(dir: &Path) -> Result<Command, String> {
    let mut command = Command::new("xdg-open");
    command.arg(dir);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn folder_command(_dir: &Path) -> Result<Command, String> {
    Err("Opening folders is not supported on this platform".to_string())
}

#[tauri::command]
fn get_server_url(state: tauri::State<ServerState>) -> Result<String, String> {
    let status = state.status.lock().unwrap();
    status.server_url.clone().ok_or_else(|| {
        status
            .error
            .clone()
            .unwrap_or_else(|| "The desktop server is still starting.".to_string())
    })
}

#[tauri::command]
fn get_desktop_server_settings(state: tauri::State<ServerState>) -> DesktopServerStatus {
    state.status.lock().unwrap().clone()
}

#[tauri::command]
async fn set_desktop_server_port(
    state: tauri::State<'_, ServerState>,
    port: Option<u16>,
) -> Result<DesktopServerStatus, String> {
    state.save_port(port).await
}

/// Wait for a dialog's answer: the plugin calls back on its own thread.
async fn dialog_answer(
    open: impl FnOnce(Box<dyn FnOnce(Option<String>) + Send>),
) -> Option<String> {
    let (sender, receiver) = oneshot::channel();
    open(Box::new(move |picked| {
        let _ = sender.send(picked);
    }));
    receiver.await.ok().flatten()
}

#[tauri::command]
async fn pick_database_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    Ok(dialog_answer(|answer| {
        app.dialog()
            .file()
            .add_filter("SQLite Database", &["sqlite", "db"])
            .add_filter("All Files", &["*"])
            .set_title("Select N.I.N.A. Database File")
            .pick_file(move |file| answer(file.map(|path| path.to_string())));
    })
    .await)
}

/// Pick a folder, or a file when `file` is set, for any path setting.
#[tauri::command]
async fn pick_folder(
    app: tauri::AppHandle,
    title: Option<String>,
    file: Option<bool>,
) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    let picks_file = file.unwrap_or(false);
    let title = title.unwrap_or_else(|| {
        if picks_file {
            "Select File".into()
        } else {
            "Select Folder".into()
        }
    });
    Ok(dialog_answer(|answer| {
        let dialog = app.dialog().file().set_title(title);
        if picks_file {
            dialog.pick_file(move |path| answer(path.map(|path| path.to_string())));
        } else {
            dialog.pick_folder(move |path| answer(path.map(|path| path.to_string())));
        }
    })
    .await)
}

#[tauri::command]
fn get_default_nina_database_path() -> Option<String> {
    get_nina_database_path()
}

fn get_nina_database_path() -> Option<String> {
    #[cfg(target_os = "windows")]
    {
        // N.I.N.A. default database location on Windows
        use std::env;

        if let Ok(localappdata) = env::var("LOCALAPPDATA") {
            let nina_path = std::path::PathBuf::from(localappdata)
                .join("NINA")
                .join("Database")
                .join("NINA.sqlite");

            if nina_path.exists() {
                return Some(nina_path.to_string_lossy().to_string());
            }
        }

        if let Ok(userprofile) = env::var("USERPROFILE") {
            let nina_path = std::path::PathBuf::from(userprofile)
                .join("AppData")
                .join("Local")
                .join("NINA")
                .join("Database")
                .join("NINA.sqlite");

            if nina_path.exists() {
                return Some(nina_path.to_string_lossy().to_string());
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        // For non-Windows platforms, no default N.I.N.A. path. Users use the file picker.
    }

    None
}

async fn start_server_for_tauri(
    socket: ReadyListener,
    databases: Vec<DbEntry>,
    astrometry_config: Option<crate::astrometry::AstrometryConfig>,
    server_config: TauriServerConfig,
    registry_path: PathBuf,
    shutdown_rx: oneshot::Receiver<()>,
    stop: Arc<Shutdown>,
) -> anyhow::Result<()> {
    let port = socket.listener.local_addr()?.port();
    let listener = tokio::net::TcpListener::from_std(socket.listener)?;
    if databases.is_empty() {
        println!("No databases configured — open settings to add one.");
    } else {
        println!("Loaded {} configured database(s):", databases.len());
        for db in &databases {
            println!("  - {} ({}): {}", db.name, db.id, db.db_path);
        }
    }

    if let Some(cache_base) = dirs::cache_dir() {
        println!("System cache directory: {}", cache_base.display());
    }
    if let Some(config_base) = dirs::config_dir() {
        println!("System config directory: {}", config_base.display());
    }

    // The desktop app deliberately binds localhost.
    let host = "127.0.0.1".to_string();

    let server_config = crate::server::ServerConfig {
        databases,
        static_dir: server_config.static_dir,
        storage: server_config.storage,
        host,
        port,
        pregeneration_config: server_config.pregeneration,
        registry_path: Some(registry_path),
        // The Tauri app is local-only and trusted; always enable CRUD so the
        // settings panel can add/remove databases without an extra flag.
        allow_database_management: true,
        // Default store beside the registry; the desktop app has no flag.
        director_meta: None,
        allow_anonymous_access: false,
        // The desktop app does not read the server TOML.
        site_banner: None,
        // Tauri binds localhost and does not use browser authentication.
        auth: None,
        worker_policy: crate::config::Config::default().get_worker_policy(),
        // The desktop app has disk to spare and no operator to ask, so it
        // keeps the exact rendition.
        preview_encoding: crate::preview_format::PreviewEncoding::png(),
        preview_color_default: true,
        keep_failed_uploads: false,
        astrometry_config,
        shutdown: Some(stop),
        config_path: None,
    };

    crate::server::run_server_on_listener(server_config, shutdown_rx, listener, socket.ready).await
}

// ── Tauri commands operating on the registry ──────────────────────────────────

fn registry_path(state: &ServerState) -> Result<PathBuf, String> {
    Ok(state
        .registry_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone())
}

fn load_registry(state: &ServerState) -> Result<DbRegistry, String> {
    DbRegistry::load_or_init(&registry_path(state)?)
        .map_err(|e| format!("Could not read the database registry: {e:#}"))
}

#[tauri::command]
fn get_current_configuration(state: tauri::State<ServerState>) -> Result<DbRegistry, String> {
    load_registry(&state)
}

#[tauri::command]
async fn show_image_in_folder(
    state: tauri::State<'_, ServerState>,
    db_id: String,
    path: String,
) -> Result<(), String> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let registry = load_registry(&state)?;
        let path = validate_image_reveal_path(&registry, &db_id, &path)?;
        launch_file_manager(&path)
    })
    .await
    .map_err(|e| format!("File manager task failed: {e}"))?
}

fn validate_image_reveal_path(
    registry: &DbRegistry,
    db_id: &str,
    requested: &str,
) -> Result<PathBuf, String> {
    if requested.trim().is_empty() {
        return Err("Image path is empty".to_string());
    }

    let database = registry
        .find(db_id)
        .ok_or_else(|| "Image catalog is not configured".to_string())?;
    let path =
        dunce::canonicalize(requested).map_err(|e| format!("Image file is not available: {e}"))?;
    if !path.is_file() {
        return Err("Image path does not point to a file".to_string());
    }

    let registered = database.image_dirs.iter().any(|root| {
        dunce::canonicalize(root)
            .map(|root| path.starts_with(root))
            .unwrap_or(false)
    });
    if !registered {
        return Err("Image file is outside the configured image folders".to_string());
    }

    Ok(path)
}

fn launch_file_manager(path: &Path) -> Result<(), String> {
    let mut command = file_manager_command(path)?;
    let status = command
        .status()
        .map_err(|e| format!("Could not open the file manager: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("The file manager exited with status {status}"))
    }
}

#[cfg(target_os = "macos")]
fn file_manager_command(path: &Path) -> Result<Command, String> {
    let mut command = Command::new("open");
    command.arg("-R").arg(path);
    Ok(command)
}

#[cfg(target_os = "windows")]
fn file_manager_command(path: &Path) -> Result<Command, String> {
    let mut command = Command::new("explorer.exe");
    command.arg(format!("/select,{}", path.display()));
    Ok(command)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn file_manager_command(path: &Path) -> Result<Command, String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Image file has no parent folder".to_string())?;
    let mut command = Command::new("xdg-open");
    command.arg(parent);
    Ok(command)
}

#[cfg(not(any(target_os = "macos", target_os = "windows", unix)))]
fn file_manager_command(_path: &Path) -> Result<Command, String> {
    Err("Showing files is not supported on this platform".to_string())
}

#[tauri::command]
async fn restart_application(app: tauri::AppHandle) -> Result<(), String> {
    app.restart();
}

#[tauri::command]
async fn restart_server(
    state: tauri::State<'_, ServerState>,
    base: tauri::State<'_, TauriServerConfig>,
) -> Result<String, String> {
    tracing::info!("🔄 Server restart requested");

    state.restart(base.inner().clone()).await
}

#[tauri::command]
fn is_configuration_valid(state: tauri::State<ServerState>) -> Result<bool, String> {
    let reg = load_registry(&state)?;
    Ok(reg
        .databases
        .iter()
        .any(|d| !d.db_path.trim().is_empty() && std::path::Path::new(&d.db_path).exists()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_registry::CURRENT_SCHEMA_VERSION;
    use tempfile::tempdir;

    fn registry_with_root(root: &Path) -> DbRegistry {
        DbRegistry {
            schema_version: CURRENT_SCHEMA_VERSION,
            databases: vec![DbEntry {
                ts_sync: None,
                id: "test".to_string(),
                name: "Test".to_string(),
                db_path: root.join("catalog.sqlite").display().to_string(),
                image_dirs: vec![root.display().to_string()],
                reject_archive: None,
                remote_image_upload: None,
                export_dir: None,
                process_dir: None,
                autoimport: None,
                scan_rules: None,
                analyze_new_frames: false,
                pair_calibrated_copies: true,
                scan_calibrated_copies: false,
            }],
            active_db_id: None,
            astrometry: None,
            calibration: None,
            export: None,
            stacking: None,
            workers: None,
            storage: None,
            astrobin: None,
            pixinsight: None,
            backups: None,
            peers: Vec::new(),
        }
    }

    #[tokio::test]
    async fn fixed_port_restarts_keep_the_http_api_and_mcp_together() {
        let dir = tempdir().unwrap();
        let registry_path = dir.path().join("config.json");
        let registry = DbRegistry::default();
        registry.save(&registry_path).unwrap();
        let listener = DesktopSettings::default().bind().unwrap();
        let port = listener.local_addr().unwrap().port();
        let state = ServerState {
            status: Arc::new(Mutex::new(DesktopServerStatus::default())),
            listener: Arc::new(Mutex::new(None)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            registry_path: Arc::new(Mutex::new(registry_path.clone())),
            server_shutdown: Arc::new(Mutex::new(None)),
            server_task: Arc::new(Mutex::new(None)),
            server_stop: Arc::new(Mutex::new(None)),
        };
        let base = TauriServerConfig {
            static_dir: None,
            pregeneration: PregenerationConfig::default(),
            storage: crate::server::storage::StorageConfig::defaulting_to(dir.path().join("cache")),
        };
        let url = state
            .start_server(listener, registry, base.clone())
            .await
            .unwrap();
        state.save_port(Some(port)).await.unwrap();
        assert_eq!(
            DesktopSettings::load(&DesktopSettings::path(&registry_path))
                .unwrap()
                .server_port,
            Some(port)
        );

        let client = reqwest::Client::builder().no_proxy().build().unwrap();
        for restart in [false, true] {
            if restart {
                assert_eq!(state.restart(base.clone()).await.unwrap(), url);
            }
            assert_eq!(
                client
                    .get(format!("{url}/api/info"))
                    .send()
                    .await
                    .unwrap()
                    .status(),
                200
            );
            assert_eq!(
                client.get(format!("{url}/")).send().await.unwrap().status(),
                200
            );
            let reply: serde_json::Value = client
                .post(format!("{url}/api/mcp"))
                .header("accept", "application/json, text/event-stream")
                .header("mcp-protocol-version", "2025-06-18")
                .json(
                    &serde_json::json!({"jsonrpc":"2.0", "id":1, "method":"tools/call",
                    "params":{"name":"list_databases", "arguments":{}}}),
                )
                .send()
                .await
                .unwrap()
                .error_for_status()
                .unwrap()
                .json()
                .await
                .unwrap();
            assert_ne!(reply["result"]["isError"], true, "{reply}");
            assert!(reply["result"]["content"].is_array(), "{reply}");
        }

        state.stop_server(RESTART_WAIT).await.unwrap();
        assert!(
            DesktopSettings {
                server_port: Some(port)
            }
            .bind()
            .is_err(),
            "the fixed socket stays reserved even after the old server ends"
        );
        assert_eq!(state.restart(base.clone()).await.unwrap(), url);

        // Occupied-port saves do not alter the persisted choice or stop the
        // working server. A port taken after a save is checked again on restart.
        let occupied = DesktopSettings::default().bind().unwrap();
        let occupied_port = occupied.local_addr().unwrap().port();
        assert!(state
            .save_port(Some(occupied_port))
            .await
            .unwrap_err()
            .contains("in use"));
        assert_eq!(
            DesktopSettings::load(&DesktopSettings::path(&registry_path))
                .unwrap()
                .server_port,
            Some(port)
        );
        DesktopSettings {
            server_port: Some(occupied_port),
        }
        .save(&DesktopSettings::path(&registry_path))
        .unwrap();
        assert!(state
            .restart(base.clone())
            .await
            .unwrap_err()
            .contains("in use"));
        assert_eq!(state.reserved_port(), Some(port));
        assert_eq!(
            client
                .get(format!("{url}/api/info"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );

        // Returning to Automatic still waits for a new owned socket and ready API.
        state.save_port(None).await.unwrap();
        let automatic_url = state.restart(base).await.unwrap();
        assert_eq!(
            client
                .get(format!("{automatic_url}/api/info"))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        state.stop_server(RESTART_WAIT).await.unwrap();
    }

    #[tokio::test]
    async fn a_restart_waits_for_the_old_server_to_finish() {
        // A server that takes a while to drain after the signal, then
        // writes its journal.
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let journal_written = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let written = Arc::clone(&journal_written);
        let task = tokio::spawn(async move {
            shutdown_rx.await.ok();
            tokio::time::sleep(Duration::from_millis(300)).await;
            written.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        let state = ServerState {
            status: Arc::new(Mutex::new(DesktopServerStatus::default())),
            listener: Arc::new(Mutex::new(None)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            registry_path: Arc::new(Mutex::new(PathBuf::new())),
            server_shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
            server_task: Arc::new(Mutex::new(Some(task))),
            server_stop: Arc::new(Mutex::new(Some(Shutdown::new()))),
        };
        state.stop_server(RESTART_WAIT).await.unwrap();
        assert!(
            journal_written.load(std::sync::atomic::Ordering::SeqCst),
            "the next server starts only after the old one's last write"
        );
        assert!(state.server_shutdown.lock().unwrap().is_none());
        assert!(state.server_task.lock().unwrap().is_none());
        // Nothing left to stop: returns at once.
        state.stop_server(RESTART_WAIT).await.unwrap();
    }

    #[tokio::test]
    async fn a_restart_gives_up_on_a_stuck_server_and_says_what_it_waits_for() {
        // A job stuck in a read from a share that stopped answering: the
        // server never ends.
        let stop = Shutdown::new();
        let stuck = stop.hold("stack build (rig)");
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            shutdown_rx.await.ok();
            std::future::pending::<()>().await;
        });
        let state = ServerState {
            status: Arc::new(Mutex::new(DesktopServerStatus::default())),
            listener: Arc::new(Mutex::new(None)),
            lifecycle: Arc::new(tokio::sync::Mutex::new(())),
            registry_path: Arc::new(Mutex::new(PathBuf::new())),
            server_shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
            server_task: Arc::new(Mutex::new(Some(task))),
            server_stop: Arc::new(Mutex::new(Some(stop))),
        };
        let error = state
            .stop_server(Duration::from_millis(100))
            .await
            .unwrap_err();
        assert!(error.contains("still finishing"), "{error}");
        assert!(error.contains("stack build (rig)"), "{error}");
        assert!(
            state.server_task.lock().unwrap().is_some(),
            "the next restart waits for it again"
        );
        drop(stuck);
    }

    #[test]
    fn reveal_path_accepts_an_existing_file_under_an_image_root() {
        let root = tempdir().unwrap();
        let file = root.path().join("lights").join("frame.fits");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"fits").unwrap();

        let validated = validate_image_reveal_path(
            &registry_with_root(root.path()),
            "test",
            file.to_str().unwrap(),
        )
        .unwrap();

        assert_eq!(validated, dunce::canonicalize(file).unwrap());
    }

    #[test]
    fn reveal_path_rejects_files_outside_registered_image_roots() {
        let root = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let file = outside.path().join("frame.fits");
        std::fs::write(&file, b"fits").unwrap();

        let error = validate_image_reveal_path(
            &registry_with_root(root.path()),
            "test",
            file.to_str().unwrap(),
        )
        .unwrap_err();

        assert_eq!(error, "Image file is outside the configured image folders");
    }

    #[test]
    fn reveal_path_rejects_missing_files_and_directories() {
        let root = tempdir().unwrap();
        let registry = registry_with_root(root.path());

        assert!(validate_image_reveal_path(
            &registry,
            "test",
            root.path().join("missing.fits").to_str().unwrap()
        )
        .unwrap_err()
        .starts_with("Image file is not available:"));
        assert_eq!(
            validate_image_reveal_path(&registry, "test", root.path().to_str().unwrap())
                .unwrap_err(),
            "Image path does not point to a file"
        );
    }

    #[test]
    fn reveal_path_rejects_unknown_catalogs() {
        let root = tempdir().unwrap();
        let file = root.path().join("frame.fits");
        std::fs::write(&file, b"fits").unwrap();

        assert_eq!(
            validate_image_reveal_path(
                &registry_with_root(root.path()),
                "other",
                file.to_str().unwrap()
            )
            .unwrap_err(),
            "Image catalog is not configured"
        );
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn file_manager_command_reveals_the_file_in_finder() {
        use std::ffi::OsStr;

        let command = file_manager_command(Path::new("/tmp/frame.fits")).unwrap();

        assert_eq!(command.get_program(), OsStr::new("open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("-R"), OsStr::new("/tmp/frame.fits")]
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn file_manager_command_selects_the_file_in_explorer() {
        use std::ffi::OsStr;

        let command = file_manager_command(Path::new(r"C:\images\frame.fits")).unwrap();

        assert_eq!(command.get_program(), OsStr::new("explorer.exe"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new(r"/select,C:\images\frame.fits")]
        );
    }

    #[test]
    #[cfg(target_os = "windows")]
    fn validated_reveal_path_uses_an_explorer_compatible_path() {
        use std::ffi::OsStr;

        let root = tempdir().unwrap();
        let file = root.path().join("frame.fits");
        std::fs::write(&file, b"fits").unwrap();
        let validated = validate_image_reveal_path(
            &registry_with_root(root.path()),
            "test",
            file.to_str().unwrap(),
        )
        .unwrap();
        let command = file_manager_command(&validated).unwrap();
        let argument = command.get_args().next().unwrap();

        assert_eq!(
            argument,
            OsStr::new(&format!("/select,{}", validated.display()))
        );
        assert!(!argument.to_string_lossy().contains(r"\\?\"));
    }

    #[test]
    #[cfg(all(unix, not(target_os = "macos")))]
    fn file_manager_command_opens_the_parent_directory() {
        use std::ffi::OsStr;

        let command = file_manager_command(Path::new("/tmp/images/frame.fits")).unwrap();

        assert_eq!(command.get_program(), OsStr::new("xdg-open"));
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            vec![OsStr::new("/tmp/images")]
        );
    }
}
