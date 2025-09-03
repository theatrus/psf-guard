use anyhow::Context;
use crate::cli::PregenerationConfig;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TauriConfig {
    pub database_path: Option<String>,
    pub image_directories: Vec<String>,
}

impl Default for TauriConfig {
    fn default() -> Self {
        Self {
            database_path: get_nina_database_path(),
            image_directories: Vec::new(),
        }
    }
}

#[derive(Clone)]
struct ServerState {
    url: String,
    config: Arc<Mutex<TauriConfig>>,
}

#[derive(Debug, Clone)]
struct TauriServerConfig {
    config_file: Option<String>,
    database_path: Option<String>,
    image_dirs: Option<Vec<String>>,
    static_dir: Option<String>,
    cache_dir: Option<String>,
    pregeneration: PregenerationConfig,
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn main() {
    // Load or create initial configuration
    let initial_config = load_configuration().unwrap_or_default();
    
    // Use configuration for server setup
    let server_config = TauriServerConfig {
        config_file: None,
        database_path: initial_config.database_path.clone(),
        image_dirs: if initial_config.image_directories.is_empty() {
            None
        } else {
            Some(initial_config.image_directories.clone())
        },
        static_dir: None,
        cache_dir: None,
        pregeneration: PregenerationConfig::default(),
    };
    
    // Create tokio runtime for the server
    let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
    
    // Find a free port and start the server
    let server_port = find_free_port().expect("Could not find free port");
    let server_url = format!("http://localhost:{}", server_port);
    
    println!("Starting PSF Guard server on {}", server_url);
    
    // Start the server in background
    rt.spawn(async move {
        if let Err(e) = start_server_for_tauri(server_port, server_config).await {
            eprintln!("Server error: {}", e);
        }
    });
    
    // Start Tauri app with the server URL and config state
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerState { 
            url: server_url,
            config: Arc::new(Mutex::new(initial_config)),
        })
        .invoke_handler(tauri::generate_handler![
            get_server_url,
            pick_database_file,
            pick_image_directory,
            get_default_nina_database_path,
            save_configuration,
            get_current_configuration,
            restart_application
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[tauri::command]
fn get_server_url(state: tauri::State<ServerState>) -> String {
    state.url.clone()
}

#[tauri::command]
async fn pick_database_file(app: tauri::AppHandle) -> Result<Option<String>, String> {
    use tauri_plugin_dialog::DialogExt;
    use std::sync::{Arc, Mutex};
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
    use tauri_plugin_dialog::DialogExt;
    use std::sync::{Arc, Mutex};
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
        
        // Alternative path
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
        // For non-Windows platforms, we don't have a default N.I.N.A. path
        // Users will need to use the file picker
    }
    
    None
}

fn find_free_port() -> anyhow::Result<u16> {
    use std::net::{TcpListener, SocketAddr};
    
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

async fn start_server_for_tauri(port: u16, server_config: TauriServerConfig) -> anyhow::Result<()> {
    use crate::config::Config;
    
    // Create default configuration for Tauri mode
    let mut config = if let Some(config_file) = server_config.config_file {
        Config::from_file(&config_file)
            .with_context(|| format!("Failed to load config file: {}", config_file))?
    } else {
        Config::default()
    };
    
    // Determine database path - try N.I.N.A. first, then fall back to temp database
    let database_path = server_config.database_path
        .or_else(|| get_nina_database_path())
        .unwrap_or_else(|| {
            // Use platform-appropriate data directory for database
            let data_dir = dirs::data_dir()
                .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
                .join("psf-guard");
            data_dir.join("temp.db").to_string_lossy().to_string()
        });
    
    // Determine cache directory - use platform-appropriate cache directory
    let cache_dir = server_config.cache_dir.unwrap_or_else(|| {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(|| {
                // Fallback to temp directory if cache dir can't be determined
                std::env::temp_dir().join("psf-guard-cache")
            })
            .join("psf-guard");
        cache_dir.to_string_lossy().to_string()
    });
    
    // Create directories if they don't exist
    if let Some(parent) = std::path::Path::new(&database_path).parent() {
        std::fs::create_dir_all(parent)?;
        println!("Created data directory: {}", parent.display());
    }
    std::fs::create_dir_all(&cache_dir)?;
    println!("Created cache directory: {}", cache_dir);
    
    // Create a minimal SQLite database if it doesn't exist
    if !std::path::Path::new(&database_path).exists() {
        println!("Creating temporary database at: {}", database_path);
        use rusqlite::Connection;
        let conn = Connection::open(&database_path)?;
        // Create minimal schema to allow the server to start
        conn.execute("CREATE TABLE IF NOT EXISTS projects (id INTEGER PRIMARY KEY, name TEXT)", [])?;
        conn.execute("CREATE TABLE IF NOT EXISTS targets (id INTEGER PRIMARY KEY, projectId INTEGER, name TEXT)", [])?;
        conn.execute("CREATE TABLE IF NOT EXISTS acquiredimage (id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, metadata TEXT)", [])?;
    }
    
    // Override config with server configuration
    config.merge_with_cli(
        Some(database_path.clone()),
        server_config.image_dirs,
        Some(port),
        Some(cache_dir.clone()),
    );
    
    config.validate()?;
    
    // Log the paths being used
    println!("Using database: {}", database_path);
    println!("Using cache directory: {}", cache_dir);
    if !config.images.directories.is_empty() {
        println!("Image directories: {}", config.images.directories.join(", "));
    } else {
        println!("No image directories configured - you can add them via the UI");
    }
    
    // Log system directory information for transparency
    if let Some(cache_base) = dirs::cache_dir() {
        println!("System cache directory: {}", cache_base.display());
    }
    if let Some(config_base) = dirs::config_dir() {
        println!("System config directory: {}", config_base.display());
    }
    
    // Clone values before move to avoid borrow checker issues
    let database_path = config.database.path.clone();
    let image_directories = config.images.directories.clone();
    let cache_directory = config.get_cache_directory();
    let host = "127.0.0.1".to_string();
    let port = config.get_port();
    
    // Start the server
    crate::server::run_server(
        database_path,
        image_directories,
        server_config.static_dir, // Use static dir if provided
        cache_directory,
        host,
        port,
        server_config.pregeneration,
    ).await
}

// Configuration management functions
fn get_config_path() -> Result<std::path::PathBuf, Box<dyn std::error::Error>> {
    let config_dir = dirs::config_dir()
        .ok_or("Could not determine config directory")?
        .join("psf-guard");
    
    std::fs::create_dir_all(&config_dir)?;
    Ok(config_dir.join("config.json"))
}

fn load_configuration() -> Result<TauriConfig, Box<dyn std::error::Error>> {
    let config_path = get_config_path()?;
    
    if config_path.exists() {
        let config_str = std::fs::read_to_string(config_path)?;
        let config: TauriConfig = serde_json::from_str(&config_str)?;
        Ok(config)
    } else {
        Ok(TauriConfig::default())
    }
}

fn save_configuration_to_file(config: &TauriConfig) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = get_config_path()?;
    let config_str = serde_json::to_string_pretty(config)?;
    std::fs::write(config_path, config_str)?;
    Ok(())
}

// Tauri command implementations
#[tauri::command]
fn get_current_configuration(state: tauri::State<ServerState>) -> Result<TauriConfig, String> {
    let config = state.config.lock().map_err(|e| e.to_string())?;
    Ok(config.clone())
}

#[tauri::command]
fn save_configuration(
    state: tauri::State<ServerState>,
    config: TauriConfig,
) -> Result<(), String> {
    // Update in-memory config
    {
        let mut current_config = state.config.lock().map_err(|e| e.to_string())?;
        *current_config = config.clone();
    }
    
    // Save to file
    save_configuration_to_file(&config).map_err(|e| e.to_string())?;
    
    Ok(())
}

#[tauri::command]
async fn restart_application(app: tauri::AppHandle) -> Result<(), String> {
    // Restart the entire Tauri application to apply new configuration
    app.restart();
}