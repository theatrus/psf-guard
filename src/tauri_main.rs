use crate::cli::PregenerationConfig;
use crate::db_registry::{DbEntry, DbRegistry};
use std::path::PathBuf;
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
