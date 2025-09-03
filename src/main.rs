#[cfg(not(feature = "tauri"))]
use anyhow::{Context, Result};
#[cfg(not(feature = "tauri"))]
use clap::Parser;
#[cfg(not(feature = "tauri"))]
use rusqlite::Connection;

#[cfg(not(feature = "tauri"))]
use psf_guard::cli::{Cli, Commands};
#[cfg(not(feature = "tauri"))]
use psf_guard::commands::{
    analyze_fits_and_compare, annotate_stars, benchmark_psf, dump_grading_results,
    filter_rejected_files, list_projects, list_targets, read_fits, regrade_images, show_images,
    stretch_to_png, update_grade,
};

#[cfg(feature = "tauri")]
use psf_guard::cli::PregenerationConfig;
#[cfg(feature = "tauri")]
use anyhow::Context;

// For Tauri builds, spawn Axum server and connect frontend to it
#[cfg(feature = "tauri")]
#[cfg_attr(mobile, tauri::mobile_entry_point)]
fn main() {
    // Use default configuration for Tauri mode - no CLI parsing
    let server_config = TauriServerConfig {
        config_file: None,
        database_path: None, // Will be determined by get_nina_database_path() or user selection
        image_dirs: None,
        static_dir: None,
        cache_dir: None,
        port: None,
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
    
    // Start Tauri app with the server URL
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(ServerState { url: server_url })
        .invoke_handler(tauri::generate_handler![
            get_server_url,
            pick_database_file,
            pick_image_directory,
            get_default_nina_database_path
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(feature = "tauri")]
#[derive(Clone)]
struct ServerState {
    url: String,
}

#[cfg(feature = "tauri")]
#[derive(Debug, Clone)]
struct TauriServerConfig {
    config_file: Option<String>,
    database_path: Option<String>,
    image_dirs: Option<Vec<String>>,
    static_dir: Option<String>,
    cache_dir: Option<String>,
    port: Option<u16>,
    pregeneration: PregenerationConfig,
}

#[cfg(feature = "tauri")]
#[tauri::command]
fn get_server_url(state: tauri::State<ServerState>) -> String {
    state.url.clone()
}

#[cfg(feature = "tauri")]
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

#[cfg(feature = "tauri")]
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

#[cfg(feature = "tauri")]
#[tauri::command]
fn get_default_nina_database_path() -> Option<String> {
    get_nina_database_path()
}

#[cfg(feature = "tauri")]
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

#[cfg(feature = "tauri")]
fn find_free_port() -> anyhow::Result<u16> {
    use std::net::{TcpListener, SocketAddr};
    
    let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

#[cfg(feature = "tauri")]
async fn start_server_for_tauri(port: u16, server_config: TauriServerConfig) -> anyhow::Result<()> {
    use psf_guard::config::Config;
    
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
            let home_dir = std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .unwrap_or_else(|_| "/tmp".to_string());
            let app_dir = std::path::PathBuf::from(home_dir).join(".psf-guard");
            app_dir.join("temp.db").to_string_lossy().to_string()
        });
    
    // Determine cache directory
    let cache_dir = server_config.cache_dir.unwrap_or_else(|| {
        let home_dir = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| "/tmp".to_string());
        let app_dir = std::path::PathBuf::from(home_dir).join(".psf-guard");
        app_dir.join("cache").to_string_lossy().to_string()
    });
    
    // Create directories if they don't exist
    if let Some(parent) = std::path::Path::new(&database_path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::create_dir_all(&cache_dir)?;
    
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
    
    // Clone values before move to avoid borrow checker issues
    let database_path = config.database.path.clone();
    let image_directories = config.images.directories.clone();
    let cache_directory = config.get_cache_directory();
    let host = "127.0.0.1".to_string();
    let port = config.get_port();
    
    // Start the server
    psf_guard::server::run_server(
        database_path,
        image_directories,
        server_config.static_dir, // Use static dir if provided
        cache_directory,
        host,
        port,
        server_config.pregeneration,
    ).await
}

// For CLI builds, use the regular CLI entry point
#[cfg(not(feature = "tauri"))]
fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::DumpGrading {
            status,
            project,
            target,
            format,
        } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            dump_grading_results(&conn, status, project, target, &format)?;
        }
        Commands::ListProjects => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            list_projects(&conn)?;
        }
        Commands::ListTargets { project } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            list_targets(&conn, &project)?;
        }
        Commands::FilterRejected {
            database,
            base_dir,
            dry_run,
            project,
            target,
            verbose,
            stat_options,
        } => {
            let conn = Connection::open(&database)
                .with_context(|| format!("Failed to open database: {}", database))?;

            let stat_config = stat_options.to_grading_config();
            filter_rejected_files(
                &conn,
                &base_dir,
                dry_run,
                project,
                target,
                stat_config,
                verbose,
            )?;
        }
        Commands::Regrade {
            database,
            dry_run,
            target,
            project,
            days,
            reset,
            stat_options,
        } => {
            let conn = Connection::open(&database)
                .with_context(|| format!("Failed to open database: {}", database))?;

            let stat_config = stat_options.to_grading_config();
            regrade_images(&conn, dry_run, target, project, days, &reset, stat_config)?;
        }
        Commands::ShowImages { ids } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            show_images(&conn, &ids)?;
        }
        Commands::UpdateGrade { id, status, reason } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            update_grade(&conn, id, &status, reason)?;
        }
        Commands::ReadFits {
            path,
            verbose,
            format,
        } => {
            read_fits(&path, verbose, &format)?;
        }
        Commands::AnalyzeFits {
            path,
            project,
            target,
            format,
            detector,
            sensitivity,
            apply_stretch,
            compare_all,
            psf_type,
            verbose,
        } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            analyze_fits_and_compare(
                &conn,
                &path,
                project,
                target,
                &format,
                &detector,
                &sensitivity,
                apply_stretch,
                compare_all,
                &psf_type,
                verbose,
            )?;
        }
        Commands::StretchToPng {
            fits_path,
            output,
            midtone_factor,
            shadow_clipping,
            logarithmic,
            invert,
        } => {
            stretch_to_png(
                &fits_path,
                output,
                midtone_factor,
                shadow_clipping,
                logarithmic,
                invert,
            )?;
        }
        Commands::AnnotateStars {
            fits_path,
            output,
            max_stars,
            detector,
            sensitivity,
            midtone_factor,
            shadow_clipping,
            annotation_color,
            psf_type,
            verbose,
        } => {
            annotate_stars(
                &fits_path,
                output,
                max_stars,
                &detector,
                &sensitivity,
                midtone_factor,
                shadow_clipping,
                &annotation_color,
                &psf_type,
                verbose,
            )?;
        }
        Commands::VisualizePsf {
            fits_path,
            output,
            star_index,
            psf_type,
            max_stars,
            selection_mode,
            sort_by,
            verbose,
        } => {
            use psf_guard::commands::visualize_psf::visualize_psf_multi;

            // If a specific star index is requested, show just that one
            let num_stars = if star_index.is_some() { 1 } else { max_stars };

            visualize_psf_multi(
                &fits_path,
                output,
                num_stars,
                &psf_type,
                &sort_by,
                3, // Default to 3 columns
                &selection_mode,
                verbose,
            )?;
        }
        Commands::VisualizePsfMulti {
            fits_path,
            output,
            num_stars,
            psf_type,
            sort_by,
            grid_cols,
            selection_mode,
            verbose,
        } => {
            use psf_guard::commands::visualize_psf::visualize_psf_multi;

            visualize_psf_multi(
                &fits_path,
                output,
                num_stars,
                &psf_type,
                &sort_by,
                grid_cols,
                &selection_mode,
                verbose,
            )?;
        }
        Commands::BenchmarkPsf {
            fits_path,
            runs,
            verbose,
        } => {
            benchmark_psf(&fits_path, runs, verbose)?;
        }
        Commands::Server {
            config,
            database,
            image_dirs,
            static_dir,
            cache_dir,
            port,
            host: _host,
            pregenerate_screen,
            pregenerate_large,
            pregenerate_original,
            pregenerate_annotated,
            pregenerate_all,
            cache_expiry,
        } => {
            use psf_guard::config::Config;

            // Load configuration from file or use defaults
            let mut app_config = if let Some(config_path) = config {
                Config::from_file(&config_path)
                    .with_context(|| format!("Failed to load config file: {}", config_path))?
            } else {
                Config::default()
            };

            // Override config with command line arguments
            let database_path = if database.is_some() || image_dirs.is_empty() {
                database
            } else {
                // If no database specified and we have image_dirs from CLI, use the default
                Some(app_config.database.path.clone())
            };

            let image_directories = if !image_dirs.is_empty() {
                Some(image_dirs)
            } else {
                None
            };

            app_config.merge_with_cli(database_path, image_directories, port, cache_dir);

            // Validate configuration
            app_config
                .validate()
                .context("Configuration validation failed")?;

            // Create pregeneration configuration
            use psf_guard::cli::PregenerationConfig;
            let pregeneration_config = if pregenerate_all
                || pregenerate_screen
                || pregenerate_large
                || pregenerate_original
                || pregenerate_annotated
            {
                // CLI flags take precedence
                PregenerationConfig::from_server_args(
                    pregenerate_screen,
                    pregenerate_large,
                    pregenerate_original,
                    pregenerate_annotated,
                    pregenerate_all,
                    &cache_expiry,
                )?
            } else {
                // Use config file settings
                PregenerationConfig::from_config(app_config.get_pregeneration())
            };

            // Clone values before use to avoid borrow checker issues
            let database_path = app_config.database.path.clone();
            let image_directories = app_config.images.directories.clone();
            let cache_directory = app_config.get_cache_directory();
            let server_host = app_config.get_host();
            let server_port = app_config.get_port();

            // Use tokio runtime for async server
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                psf_guard::server::run_server(
                    database_path,
                    image_directories,
                    static_dir,
                    cache_directory,
                    server_host,
                    server_port,
                    pregeneration_config,
                )
                .await
            })?;
        }
    }

    Ok(())
}
