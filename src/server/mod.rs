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
    // Initialize tracing
    tracing_subscriber::fmt::init();

    // Create cache directory if it doesn't exist
    std::fs::create_dir_all(&cache_dir)?;

    // Create app state
    let state = match AppState::new(database_path, image_dir, cache_dir.clone()) {
        Ok(state) => Arc::new(state),
        Err(e) => {
            eprintln!("Failed to initialize server: {}", e);
            return Err(e);
        }
    };

    // Create API routes
    let api_routes = Router::new()
        .route("/info", get(handlers::get_server_info))
        .route("/refresh-cache", put(handlers::refresh_file_cache))
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

    tracing::info!("Server listening on http://{}:{}", host, port);
    tracing::info!("Using cache directory: {}", cache_dir);

    // Run server
    axum::serve(listener, app).await?;

    Ok(())
}
