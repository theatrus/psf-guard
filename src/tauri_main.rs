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
    cache_dir: Option<String>,
    pregeneration: PregenerationConfig,
}

#[derive(Clone)]
struct ServerState {
    url: Arc<Mutex<String>>,
    /// Mirror of the on-disk registry. The Tauri commands keep this in sync
    /// with `<config>/config.json` (default location).
    registry: Arc<Mutex<DbRegistry>>,
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
        cache_dir: None,
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
        .manage(ServerState {
            url: Arc::new(Mutex::new(server_url)),
            registry: Arc::new(Mutex::new(initial_registry)),
            registry_path: Arc::new(Mutex::new(registry_path)),
            server_shutdown: Arc::new(Mutex::new(Some(shutdown_tx))),
        })
        .manage(server_config)
        .invoke_handler(tauri::generate_handler![
            get_server_url,
            pick_database_file,
            pick_image_directory,
            get_default_nina_database_path,
            save_configuration,
            get_current_configuration,
            add_database,
            remove_database,
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

#[tauri::command]
async fn pick_database_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use std::sync::{Arc, Mutex};
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::Notify;

    let result = Arc::new(Mutex::new(None));
    let notify = Arc::new(Notify::new());
    let result_clone = result.clone();
    let notify_clone = notify.clone();

    app.dialog()
        .file()
        .add_filter("SQLite Database", &["sqlite", "db"])
        .add_filter("All Files", &["*"])
        .set_title("Select N.I.N.A. Database File")
        .pick_file(move |file_path| {
            *result_clone.lock().unwrap() = file_path.map(|p| p.to_string());
            notify_clone.notify_one();
        });

    notify.notified().await;
    let path = result.lock().unwrap().clone();
    Ok(path)
}

#[tauri::command]
async fn pick_image_directory(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use std::sync::{Arc, Mutex};
    use tauri_plugin_dialog::DialogExt;
    use tokio::sync::Notify;

    let result = Arc::new(Mutex::new(None));
    let notify = Arc::new(Notify::new());
    let result_clone = result.clone();
    let notify_clone = notify.clone();

    app.dialog()
        .file()
        .set_title("Select Image Directory")
        .pick_folder(move |folder_path| {
            *result_clone.lock().unwrap() = folder_path.map(|p| p.to_string());
            notify_clone.notify_one();
        });

    notify.notified().await;
    let path = result.lock().unwrap().clone();
    Ok(path)
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
    use crate::config::Config;

    let mut config = Config::default();

    let cache_dir = server_config.cache_dir.clone().unwrap_or_else(|| {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| std::env::temp_dir().join("psf-guard-cache"))
            .join("psf-guard");
        cache_dir.to_string_lossy().to_string()
    });
    std::fs::create_dir_all(&cache_dir)?;
    println!("Cache directory: {}", cache_dir);

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

    // Tauri server has no TOML config; pull port/host from defaults overridden
    // by what we computed above.
    // (host: the desktop app deliberately binds localhost below, not the config.)
    config.merge_with_cli(None, None, Some(port), None, Some(cache_dir.clone()));
    let host = "127.0.0.1".to_string();

    let server_config = crate::server::ServerConfig {
        databases,
        static_dir: server_config.static_dir,
        cache_dir,
        host,
        port,
        pregeneration_config: server_config.pregeneration,
        registry_path: Some(registry_path),
        // The Tauri app is local-only and trusted; always enable CRUD so the
        // settings panel can add/remove databases without an extra flag.
        allow_database_management: true,
        // The desktop app does not read the server TOML.
        site_banner: None,
        worker_policy: config.get_worker_policy(),
        astrometry_config,
    };

    crate::server::run_server_with_shutdown(server_config, shutdown_rx).await
}

// ── Tauri commands operating on the registry ──────────────────────────────────

#[tauri::command]
fn get_current_configuration(state: tauri::State<ServerState>) -> Result<DbRegistry, String> {
    Ok(state.registry.lock().map_err(|e| e.to_string())?.clone())
}

/// Replace the entire registry with the supplied one. Used by the multi-DB
/// settings panel (F3). Atomically writes to disk.
#[tauri::command]
fn save_configuration(state: tauri::State<ServerState>, config: DbRegistry) -> Result<(), String> {
    let path = state
        .registry_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    config.save(&path).map_err(|e| e.to_string())?;
    *state.registry.lock().map_err(|e| e.to_string())? = config;
    Ok(())
}

/// Add a single database to the registry. Returns the persisted entry, including
/// any auto-generated or disambiguated slug.
#[tauri::command]
fn add_database(
    state: tauri::State<ServerState>,
    name: String,
    db_path: String,
    image_dirs: Vec<String>,
) -> Result<DbEntry, String> {
    let path = state
        .registry_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let mut reg = state.registry.lock().map_err(|e| e.to_string())?;
    let entry = reg
        .add(name, db_path, image_dirs, None)
        .map_err(|e| e.to_string())?
        .clone();
    reg.save(&path).map_err(|e| e.to_string())?;
    Ok(entry)
}

#[tauri::command]
fn remove_database(state: tauri::State<ServerState>, db_id: String) -> Result<bool, String> {
    let path = state
        .registry_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    let mut reg = state.registry.lock().map_err(|e| e.to_string())?;
    let removed = reg.remove(&db_id).map_err(|e| e.to_string())?;
    reg.save(&path).map_err(|e| e.to_string())?;
    Ok(removed)
}

#[tauri::command]
async fn show_image_in_folder(
    state: tauri::State<'_, ServerState>,
    db_id: String,
    path: String,
) -> Result<(), String> {
    // Database CRUD runs through the local HTTP server, so the Tauri-side
    // in-memory mirror can lag behind. Reload the small registry file before
    // validating the requested path.
    let registry_path = state
        .registry_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
    tokio::task::spawn_blocking(move || {
        let registry = DbRegistry::load_or_init(&registry_path)
            .map_err(|e| format!("Could not read the database registry: {e:#}"))?;
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

    let (databases, astrometry_config) = {
        let registry = state.registry.lock().map_err(|e| e.to_string())?;
        (registry.databases.clone(), registry.astrometry.clone())
    };

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
    let registry_path = state
        .registry_path
        .lock()
        .map_err(|e| e.to_string())?
        .clone();
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
    let reg = state.registry.lock().map_err(|e| e.to_string())?;
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
            }],
            active_db_id: None,
            astrometry: None,
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
