pub mod api;
pub mod cache;
pub mod embedded_static;
pub mod handlers;
pub mod state;
pub mod static_file_service;

use anyhow::Result;
use axum::{
    routing::{get, put},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::server::embedded_static::serve_embedded_file;
use crate::server::static_file_service::StaticFileService;

use crate::cli::PregenerationConfig;
use crate::server::state::AppState;
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub database_path: String,
    pub image_dirs: Vec<String>,
    pub static_dir: Option<String>,
    pub cache_dir: String,
    pub host: String,
    pub port: u16,
    pub pregeneration_config: PregenerationConfig,
}

pub async fn run_server(
    database_path: String,
    image_dirs: Vec<String>,
    static_dir: Option<String>,
    cache_dir: String,
    host: String,
    port: u16,
    pregeneration_config: PregenerationConfig,
) -> anyhow::Result<()> {
    // Initialize tracing with environment-based filtering (for CLI mode)
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

    let config = ServerConfig {
        database_path,
        image_dirs,
        static_dir,
        cache_dir,
        host,
        port,
        pregeneration_config,
    };

    run_server_internal(config, None).await
}

pub async fn run_server_with_config(config: ServerConfig) -> anyhow::Result<()> {
    // Initialize tracing with environment-based filtering (for CLI mode)
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

    run_server_internal(config, None).await
}

async fn run_server_internal(
    config: ServerConfig,
    shutdown_rx: Option<oneshot::Receiver<()>>,
) -> anyhow::Result<()> {
    tracing::info!("🚀 Starting PSF Guard server");
    tracing::info!("📊 Database: {}", config.database_path);
    tracing::info!("📁 Image directories: {}", config.image_dirs.join(", "));
    tracing::info!("💾 Cache directory: {}", config.cache_dir);

    // Log pregeneration configuration
    if config.pregeneration_config.is_enabled() {
        let enabled_formats = config.pregeneration_config.enabled_formats();
        tracing::info!(
            "🎨 Background pre-generation enabled for: {} (cache expiry: {})",
            enabled_formats.join(", "),
            humantime::format_duration(config.pregeneration_config.cache_expiry)
        );
    } else {
        tracing::info!("🎨 Background pre-generation disabled");
    }

    // Create cache directory if it doesn't exist
    std::fs::create_dir_all(&config.cache_dir)?;

    // Create app state
    let state = match AppState::new(
        config.database_path.clone(),
        config.image_dirs.clone(),
        config.cache_dir.clone(),
        config.pregeneration_config.clone(),
    ) {
        Ok(state) => {
            tracing::info!("✅ Application state initialized successfully");
            Arc::new(state)
        }
        Err(e) => {
            tracing::error!("❌ Failed to initialize server: {}", e);
            return Err(e);
        }
    };

    // Start background cache refresh at server startup
    // This ensures the singleton refresh is started immediately
    let startup_status = state.ensure_cache_available();
    match startup_status {
        crate::server::state::RefreshStatus::InProgressWait
        | crate::server::state::RefreshStatus::InProgressServeStale => {
            tracing::info!("🔄 Cache refresh started at server startup");
        }
        crate::server::state::RefreshStatus::NotNeeded => {
            tracing::info!("✅ Cache is already available at startup");
        }
        crate::server::state::RefreshStatus::NeedsRefresh => {
            tracing::warn!("⚠️ Cache refresh needed but not started - this shouldn't happen");
        }
    }

    // Start background image pre-generation if enabled
    if config.pregeneration_config.is_enabled() {
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            background_pregeneration_task(state_clone).await;
        });
    }

    // Create API routes
    let api_routes = Router::new()
        .route("/info", get(handlers::get_server_info))
        .route("/refresh-cache", put(handlers::refresh_file_cache))
        .route("/cache-progress", get(handlers::get_cache_refresh_progress))
        .route(
            "/refresh-directory-cache",
            put(handlers::refresh_directory_tree_cache),
        )
        .route("/projects", get(handlers::list_projects))
        .route("/projects/overview", get(handlers::get_projects_overview))
        .route("/targets/overview", get(handlers::get_targets_overview))
        .route("/stats/overall", get(handlers::get_overall_stats))
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
    let app = if let Some(static_dir_path) = &config.static_dir {
        // Use filesystem static serving (for development) with proper MIME types
        let static_path = PathBuf::from(static_dir_path);
        let static_service = StaticFileService::new(static_path);

        tracing::info!("Serving static files from filesystem: {}", static_dir_path);

        Router::new()
            .nest("/api", api_routes)
            .fallback_service(static_service)
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
    let listener = tokio::net::TcpListener::bind(format!("{}:{}", config.host, config.port)).await?;

    tracing::info!("🌐 Server listening on http://{}:{}", config.host, config.port);
    tracing::info!(
        "🔧 Environment: RUST_LOG={}",
        std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string())
    );
    tracing::info!("🎯 Ready to serve requests!");

    // Run server with optional graceful shutdown
    match shutdown_rx {
        Some(shutdown_rx) => {
            tracing::info!("🚀 Server started with graceful shutdown support");
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    shutdown_rx.await.ok();
                    tracing::info!("🛑 Graceful shutdown signal received");
                })
                .await?;
        }
        None => {
            axum::serve(listener, app).await?;
        }
    }

    tracing::info!("🛑 Server shutdown completed");
    Ok(())
}

pub async fn run_server_with_shutdown(
    config: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // Don't initialize tracing here - it should already be initialized by the first server or Tauri app
    run_server_internal(config, Some(shutdown_rx)).await
}

async fn background_pregeneration_task(state: Arc<AppState>) {
    use std::time::Duration;
    use tokio::time::{interval, sleep};

    tracing::info!("🎨 Starting background image pre-generation task");

    // Rate limiting configuration
    let rate_limit_delay = Duration::from_millis(500); // 2 images per second max
    let batch_size = 10; // Process 10 images at a time
    let scan_interval = Duration::from_secs(300); // Re-scan every 5 minutes

    let mut interval_timer = interval(scan_interval);

    loop {
        // Wait for next scan interval
        interval_timer.tick().await;

        tracing::debug!("🔍 Scanning for images needing pre-generation");

        // Get all images from database
        let images = match get_all_images_for_pregeneration(&state).await {
            Ok(images) => images,
            Err(e) => {
                tracing::error!("❌ Failed to get images for pre-generation: {}", e);
                continue;
            }
        };

        if images.is_empty() {
            tracing::debug!("📭 No images found for pre-generation");
            continue;
        }

        tracing::info!("🎯 Found {} images for pre-generation", images.len());

        // Process images in batches with rate limiting
        let mut processed_count = 0;
        let mut generated_count = 0;
        let mut skipped_count = 0;
        let mut error_count = 0;

        for batch in images.chunks(batch_size) {
            for (image_id, file_only, target_name) in batch {
                let mut _formats_processed = 0;

                // Pre-generate each enabled format
                if state.pregeneration_config.screen_enabled {
                    match pregenerate_preview(&state, *image_id, file_only, target_name, "screen")
                        .await
                    {
                        Ok(generated) => {
                            _formats_processed += 1;
                            if generated {
                                generated_count += 1;
                            } else {
                                skipped_count += 1;
                            }
                        }
                        Err(e) => {
                            error_count += 1;
                            tracing::warn!(
                                "⚠️ Failed to pre-generate screen preview for image {}: {}",
                                image_id,
                                e
                            );
                        }
                    }
                }

                if state.pregeneration_config.large_enabled {
                    match pregenerate_preview(&state, *image_id, file_only, target_name, "large")
                        .await
                    {
                        Ok(generated) => {
                            _formats_processed += 1;
                            if generated {
                                generated_count += 1;
                            } else {
                                skipped_count += 1;
                            }
                        }
                        Err(e) => {
                            error_count += 1;
                            tracing::warn!(
                                "⚠️ Failed to pre-generate large preview for image {}: {}",
                                image_id,
                                e
                            );
                        }
                    }
                }

                if state.pregeneration_config.original_enabled {
                    match pregenerate_preview(&state, *image_id, file_only, target_name, "original")
                        .await
                    {
                        Ok(generated) => {
                            _formats_processed += 1;
                            if generated {
                                generated_count += 1;
                            } else {
                                skipped_count += 1;
                            }
                        }
                        Err(e) => {
                            error_count += 1;
                            tracing::warn!(
                                "⚠️ Failed to pre-generate original preview for image {}: {}",
                                image_id,
                                e
                            );
                        }
                    }
                }

                if state.pregeneration_config.annotated_enabled {
                    match pregenerate_annotated(&state, *image_id, file_only, target_name).await {
                        Ok(generated) => {
                            _formats_processed += 1;
                            if generated {
                                generated_count += 1;
                            } else {
                                skipped_count += 1;
                            }
                        }
                        Err(e) => {
                            error_count += 1;
                            tracing::warn!(
                                "⚠️ Failed to pre-generate annotated image for image {}: {}",
                                image_id,
                                e
                            );
                        }
                    }
                }

                processed_count += 1;

                // Rate limiting delay
                sleep(rate_limit_delay).await;
            }

            tracing::debug!(
                "📈 Pre-generation progress: {}/{} images processed",
                processed_count,
                images.len()
            );
        }

        if processed_count > 0 {
            tracing::info!(
                "✅ Pre-generation cycle complete: {} generated, {} skipped, {} errors ({} images processed)",
                generated_count, skipped_count, error_count, processed_count
            );
        }
    }
}

async fn get_all_images_for_pregeneration(
    state: &Arc<AppState>,
) -> Result<Vec<(i32, String, String)>> {
    use crate::db::Database;

    // Get all images from database
    let images = {
        let conn = state.db();
        let conn = conn
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock error"))?;
        let db = Database::new(&conn);

        db.query_images(None, None, None, None)
            .map_err(|_| anyhow::anyhow!("Database query error"))?
    };

    let mut result = Vec::new();

    for (image, _project_name, target_name) in images {
        // Extract filename from metadata
        if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
            if let Some(filename_path) = metadata["FileName"].as_str() {
                let file_only = filename_path
                    .split(&['\\', '/'][..])
                    .next_back()
                    .unwrap_or(filename_path)
                    .to_string();

                result.push((image.id, file_only, target_name));
            }
        }
    }

    Ok(result)
}

async fn pregenerate_preview(
    state: &Arc<AppState>,
    image_id: i32,
    file_only: &str,
    target_name: &str,
    size: &str,
) -> Result<bool> {
    use crate::server::cache::CacheManager;

    // Get image data from database first (needed for cache key)
    let image_data = {
        use crate::db::Database;

        let conn = state.db();
        let conn = conn
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock error"))?;
        let db = Database::new(&conn);

        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| anyhow::anyhow!("Database query error"))?;

        images
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Image not found: {}", image_id))?
    };

    // Create cache key matching the on-demand format for consistency
    let cache_key = format!(
        "{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image_id,
        image_data.project_id,
        image_data.target_id,
        image_data.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        "stretch", // Pre-generation always uses stretch mode
        2000,      // midtone 0.2 * 10000
        -28000     // shadow -2.8 * 10000
    );

    let cache_manager = CacheManager::new(std::path::PathBuf::from(&state.cache_dir));
    cache_manager.ensure_category_dir("previews")?;
    let cache_path = cache_manager.get_cached_path("previews", &cache_key, "png");

    // Skip if already cached and not expired
    if cache_manager.is_cached(&cache_path) {
        if let Ok(metadata) = tokio::fs::metadata(&cache_path).await {
            let age = metadata.modified()?.elapsed().unwrap_or_default();
            if age < state.pregeneration_config.cache_expiry {
                tracing::trace!(
                    "⏭️ Skipping pre-generation for image {} ({}): already cached",
                    image_id,
                    size
                );
                return Ok(false); // Skipped, not generated
            }
        }
    }

    tracing::debug!("🎨 Pre-generating {} preview for image {}", size, image_id);

    // Find FITS file using existing function
    let fits_path = handlers::find_fits_file(state, &image_data, target_name, file_only)
        .map_err(|_| anyhow::anyhow!("FITS file not found for image {}", image_id))?;

    // Determine target dimensions
    let max_dimensions = match size {
        "large" => Some((2000, 2000)),
        "screen" => Some((1200, 1200)),
        "original" => None,
        _ => Some((1200, 1200)),
    };

    // Generate preview using existing stretch function
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let cache_path_str = cache_path.to_string_lossy().to_string();

    tokio::task::spawn_blocking(move || {
        use crate::commands::stretch_to_png::stretch_to_png_with_resize;

        stretch_to_png_with_resize(
            &fits_path_str,
            Some(cache_path_str),
            0.2,   // midtone
            -2.8,  // shadow
            false, // logarithmic
            false, // invert
            max_dimensions,
        )
    })
    .await??;

    tracing::trace!("✅ Generated {} preview for image {}", size, image_id);
    Ok(true) // Successfully generated
}

async fn pregenerate_annotated(
    state: &Arc<AppState>,
    image_id: i32,
    file_only: &str,
    target_name: &str,
) -> Result<bool> {
    use crate::commands::annotate_stars_common::create_annotated_image;
    use crate::image_analysis::FitsImage;
    use crate::server::cache::CacheManager;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ColorType, ImageEncoder, Rgb};

    // Get image data from database first (needed for cache key)
    let image_data = {
        use crate::db::Database;

        let conn = state.db();
        let conn = conn
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock error"))?;
        let db = Database::new(&conn);

        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| anyhow::anyhow!("Database query error"))?;

        images
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("Image not found: {}", image_id))?
    };

    // Create cache key matching the on-demand annotated format for consistency
    let size = "screen"; // Pre-generation uses screen size for annotated images
    let max_stars = 1000; // Pre-generation uses default max_stars
    let cache_key = format!(
        "annotated_{}_{}_{}_{}_{}_{}_{}",
        image_id,
        image_data.project_id,
        image_data.target_id,
        image_data.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        max_stars
    );

    let cache_manager = CacheManager::new(std::path::PathBuf::from(&state.cache_dir));
    cache_manager.ensure_category_dir("annotated")?;
    let cache_path = cache_manager.get_cached_path("annotated", &cache_key, "png");

    // Skip if already cached and not expired
    if cache_manager.is_cached(&cache_path) {
        if let Ok(metadata) = tokio::fs::metadata(&cache_path).await {
            let age = metadata.modified()?.elapsed().unwrap_or_default();
            if age < state.pregeneration_config.cache_expiry {
                tracing::trace!(
                    "⏭️ Skipping annotated pre-generation for image {}: already cached",
                    image_id
                );
                return Ok(false); // Skipped, not generated
            }
        }
    }

    tracing::debug!("🎨 Pre-generating annotated image for image {}", image_id);

    // Find FITS file
    let fits_path = handlers::find_fits_file(state, &image_data, target_name, file_only)
        .map_err(|_| anyhow::anyhow!("FITS file not found for image {}", image_id))?;

    // Generate annotated image
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let cache_path_clone = cache_path.clone();

    tokio::task::spawn_blocking(move || -> Result<()> {
        // Load FITS file
        let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))?;

        // Create annotated image
        let rgb_image = create_annotated_image(
            &fits,
            max_stars,          // Use the same max_stars as cache key
            0.2,                // midtone_factor
            -2.8,               // shadow_clipping
            Rgb([255, 255, 0]), // yellow color
        )?;

        // Save to cache
        let cache_file = std::fs::File::create(&cache_path_clone)?;
        let writer = std::io::BufWriter::new(cache_file);
        let encoder =
            PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

        encoder.write_image(
            &rgb_image,
            rgb_image.width(),
            rgb_image.height(),
            ColorType::Rgb8.into(),
        )?;

        Ok::<(), anyhow::Error>(())
    })
    .await??;

    tracing::trace!("✅ Generated annotated image for image {}", image_id);
    Ok(true) // Successfully generated
}
