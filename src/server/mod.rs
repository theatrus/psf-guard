pub mod api;
pub mod cache;
pub mod embedded_static;
pub mod handlers;
pub mod state;

use axum::{
    routing::{get, put},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{
    cors::CorsLayer,
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};

use crate::server::embedded_static::serve_embedded_file;

use crate::server::state::AppState;

pub async fn run_server(
    database_path: String,
    image_dir: String,
    static_dir: Option<String>,
    cache_dir: String,
    host: String,
    port: u16,
) -> anyhow::Result<()> {
    // Initialize tracing with environment-based filtering
    // Set RUST_LOG=debug for debug logs, RUST_LOG=info for info logs, etc.
    // Default to info level if no RUST_LOG is set
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::filter::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::filter::EnvFilter::new("info")),
        )
        .with_target(false) // Don't show module paths in logs
        .with_level(true) // Show log levels
        .with_thread_ids(false) // Don't show thread IDs for cleaner output
        .init();

    tracing::info!("🚀 Starting PSF Guard server");
    tracing::info!("📊 Database: {}", database_path);
    tracing::info!("📁 Image directory: {}", image_dir);
    tracing::info!("💾 Cache directory: {}", cache_dir);

    // Create cache directory if it doesn't exist
    std::fs::create_dir_all(&cache_dir)?;

    // Create app state
    let state = match AppState::new(database_path.clone(), image_dir.clone(), cache_dir.clone()) {
        Ok(state) => {
            tracing::info!("✅ Application state initialized successfully");
            Arc::new(state)
        }
        Err(e) => {
            tracing::error!("❌ Failed to initialize server: {}", e);
            return Err(e);
        }
    };

    // Start background cache refresh (non-blocking)
    let state_clone = Arc::clone(&state);
    tokio::spawn(async move {
        tracing::info!("🔄 Starting background cache refresh...");
        
        // Build directory tree cache first (this is fast and needed for file finding)
        if let Err(e) = state_clone.rebuild_directory_tree() {
            tracing::warn!("⚠️ Directory tree cache build failed: {:?}", e);
        } else {
            if let Some(stats) = state_clone.get_directory_tree_stats() {
                tracing::info!(
                    "✅ Directory tree cache built - {} files, {} directories",
                    stats.total_files,
                    stats.total_directories
                );
            }
        }
        
        // Then refresh project file existence cache
        if let Err(e) = handlers::refresh_project_cache(&state_clone).await {
            tracing::warn!("⚠️ Project cache refresh failed: {:?}", e);
            tracing::info!("📝 Cache will be refreshed on first request");
        } else {
            let (projects_checked, projects_with_files) = {
                let cache = state_clone.file_check_cache.read().unwrap();
                let total = cache.projects_with_files.len();
                let found = cache.projects_with_files.values().filter(|&&v| v).count();
                (total, found)
            };
            tracing::info!(
                "✅ Background cache refresh completed - {}/{} projects have files",
                projects_with_files,
                projects_checked
            );
        }
    });

    // Create API routes
    let api_routes = Router::new()
        .route("/info", get(handlers::get_server_info))
        .route("/refresh-cache", put(handlers::refresh_file_cache))
        .route("/refresh-directory-cache", put(handlers::refresh_directory_tree_cache))
        .route("/projects", get(handlers::list_projects))
        .route(
            "/projects/{project_id}/targets",
            get(handlers::list_targets),
        )
        .route("/images", get(handlers::get_images))
        .route("/images/{image_id}", get(handlers::get_image))
        .route(
            "/images/{image_id}/preview",
            get(handlers::get_image_preview),
        )
        .route("/images/{image_id}/stars", get(handlers::get_image_stars))
        .route(
            "/images/{image_id}/annotated",
            get(handlers::get_annotated_image),
        )
        .route(
            "/images/{image_id}/psf",
            get(handlers::get_psf_visualization),
        )
        .route(
            "/images/{image_id}/grade",
            put(handlers::update_image_grade),
        )
        .with_state(state);

    // Create main app with either embedded or filesystem static serving
    let app = if let Some(static_dir_path) = &static_dir {
        // Use filesystem static serving (for development)
        let static_path = PathBuf::from(static_dir_path);
        let index_path = static_path.join("index.html");
        let serve_dir = ServeDir::new(&static_path).not_found_service(ServeFile::new(&index_path));

        tracing::info!("Serving static files from filesystem: {}", static_dir_path);

        Router::new()
            .nest("/api", api_routes)
            .fallback_service(serve_dir)
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
    } else {
        // Use embedded static serving (for production)
        tracing::info!("Serving static files from embedded assets");

        Router::new()
            .nest("/api", api_routes)
            .fallback(serve_embedded_file)
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
    };

    // Create listener
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", host, port)).await?;

    tracing::info!("🌐 Server listening on http://{}:{}", host, port);
    tracing::info!(
        "🔧 Environment: RUST_LOG={}",
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string())
    );
    tracing::info!("🎯 Ready to serve requests!");

    // Run server
    axum::serve(listener, app).await?;

    tracing::info!("🛑 Server shutdown");
    Ok(())
}
