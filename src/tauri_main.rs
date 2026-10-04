use crate::cli::PregenerationConfig;
use crate::db_registry::{DbEntry, DbRegistry};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use tokio::sync::oneshot;
use tracing_subscriber;

/// Tauri-side server bootstrap parameters. Built once at startup; rebuilt
/// (with the latest registry contents) on `restart_server`.
#[derive(Debug, Clone)]
struct TauriServerConfig {
    static_dir: Option<String>,
    pregeneration: PregenerationConfig,
}

#[derive(Clone)]
struct ServerState {
    url: Arc<Mutex<String>>,
    /// The registry file. Commands read it from disk each time: Settings
    /// save through the local server, so no in-memory copy stays current.
    registry_path: Arc<Mutex<PathBuf>>,
    server_shutdown: Arc<Mutex<Option<oneshot::Sender<()>>>>,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn main() {
    // Initialize tracing once for the entire Tauri application
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info")),
        )
        .with_target(false)
        .with_level(true)
        .with_thread_ids(false)
        .init();

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
    };

    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");

    let server_port = find_free_port().expect("Could not find free port");
    let server_url = format!("http://localhost:{}", server_port);
    println!("Starting PSF Guard server on {}", server_url);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let server_databases = initial_registry.databases.clone();
    let server_astrometry = initial_registry.astrometry.clone();
    let server_config_for_task = server_config.clone();
    let registry_path_for_task = registry_path.clone();
    rt.spawn(async move {
        if let Err(e) = start_server_for_tauri(
            server_port,
            server_databases,
            server_astrometry,
            server_config_for_task,
            registry_path_for_task,
            shutdown_rx,
        )
        .await
        {
            eprintln!("Server error: {}", e);
        }
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(ServerState {
            url: Arc::new(Mutex::new(server_url)),
            registry_path: Arc::new(Mutex::new(registry_path)),
            server_shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
        })
        .manage(server_config)
        .invoke_handler(tauri::generate_handler![
            get_server_url,
            pick_database_file,
            pick_folder,
            get_default_nina_database_path,
            get_current_configuration,
            show_image_in_folder,
            restart_application,
            restart_server,
            is_configuration_valid
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn get_server_url(state: tauri::State<ServerState>) -> String {
    state.url.lock().unwrap().clone()
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

fn find_free_port() -> anyhow::Result<u16> {
    use std::net::{SocketAddr, TcpListener};

    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn start_server_for_tauri(
    port: u16,
    databases: Vec<DbEntry>,
    astrometry_config: Option<crate::astrometry::AstrometryConfig>,
    server_config: TauriServerConfig,
    registry_path: PathBuf,
    shutdown_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // No config file: Settings choose every folder, and the cache defaults
    // to the platform cache folder.
    let default_cache = dirs::cache_dir()
        .unwrap_or_else(|| std::env::temp_dir().join("psf-guard-cache"))
        .join("psf-guard");

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
        storage: crate::server::storage::StorageConfig::defaulting_to(default_cache),
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
    };

    crate::server::run_server_with_shutdown(server_config, shutdown_rx).await
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
    _app: tauri::AppHandle,
    state: tauri::State<'_, ServerState>,
    base: tauri::State<'_, TauriServerConfig>,
) -> Result<String, String> {
    tracing::info!("🔄 Server restart requested");

    // Settings save through the server, which writes the file.
    let registry_path = registry_path(&state)?;
    let registry = load_registry(&state)?;
    let (databases, astrometry_config) = (registry.databases, registry.astrometry);

    {
        let mut shutdown_guard = state.server_shutdown.lock().unwrap();
        if let Some(shutdown_tx) = shutdown_guard.take()
            && shutdown_tx.send(()).is_err()
        {
            tracing::warn!("Failed to send shutdown signal");
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

    let server_port = find_free_port().map_err(|e| format!("Could not find free port: {}", e))?;
    let server_url = format!("http://localhost:{}", server_port);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    {
        let mut url_guard = state.url.lock().unwrap();
        *url_guard = server_url.clone();
    }
    {
        let mut shutdown_guard = state.server_shutdown.lock().unwrap();
        *shutdown_guard = Some(shutdown_tx);
    }

    tracing::info!("🚀 Starting new server on {}", server_url);

    let base_clone = base.inner().clone();
    tokio::spawn(async move {
        if let Err(e) = start_server_for_tauri(
            server_port,
            databases,
            astrometry_config,
            base_clone,
            registry_path,
            shutdown_rx,
        )
        .await
        {
            eprintln!("Server restart error: {}", e);
        }
    });

    Ok(format!("Server restarted successfully on {}", server_url))
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
                id: "test".to_string(),
                name: "Test".to_string(),
                db_path: root.join("catalog.sqlite").display().to_string(),
                image_dirs: vec![root.display().to_string()],
                reject_archive: None,
                remote_image_upload: None,
                export_dir: None,
                process_dir: None,
                autoimport: None,
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
            peers: Vec::new(),
        }
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
