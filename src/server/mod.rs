pub mod api;
pub mod cache;
pub mod database_context;
pub mod embedded_static;
pub mod extract;
pub mod handlers;
pub mod import_job;
pub mod preview_queue;
pub mod slug;
pub mod spatial_scan;
pub mod stack_preview;
pub mod state;
pub mod static_file_service;

use anyhow::{Context, Result};
use axum::{
    routing::{get, post, put},
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
    /// Every database the server should load at startup. Empty means the
    /// server still runs (the UI shows an empty state).
    pub databases: Vec<crate::db_registry::DbEntry>,
    pub static_dir: Option<String>,
    pub cache_dir: String,
    pub host: String,
    pub port: u16,
    pub pregeneration_config: PregenerationConfig,
    /// Path of the on-disk registry that mirrors `databases`. When set, the
    /// CRUD endpoints (`POST/PUT/DELETE /api/databases/...`) persist runtime
    /// changes here. `None` disables those endpoints.
    pub registry_path: Option<PathBuf>,
    /// Allow HTTP clients to mutate the configured database list. Off by
    /// default for CLI servers; Tauri always enables it.
    pub allow_database_management: bool,
    /// Tuning policy for the parallel scans and background pre-generation.
    /// See `concurrency::WorkerPolicy`.
    pub worker_policy: crate::concurrency::WorkerPolicy,
    /// Process-global Seiza catalog configuration from the shared registry.
    pub astrometry_config: Option<crate::astrometry::AstrometryConfig>,
}

#[allow(clippy::too_many_arguments)]
pub async fn run_server(
    databases: Vec<crate::db_registry::DbEntry>,
    static_dir: Option<String>,
    cache_dir: String,
    host: String,
    port: u16,
    pregeneration_config: PregenerationConfig,
    registry_path: Option<PathBuf>,
    allow_database_management: bool,
    worker_policy: crate::concurrency::WorkerPolicy,
    astrometry_config: Option<crate::astrometry::AstrometryConfig>,
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
        databases,
        static_dir,
        cache_dir,
        host,
        port,
        pregeneration_config,
        registry_path,
        allow_database_management,
        worker_policy,
        astrometry_config,
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
    tracing::info!(
        "📊 Databases ({}):{}",
        config.databases.len(),
        config
            .databases
            .iter()
            .map(|d| format!("\n   - {} ({}): {}", d.name, d.id, d.db_path))
            .collect::<String>()
    );
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
    let state = match AppState::from_databases_with_astrometry(
        config.databases.clone(),
        config.cache_dir.clone(),
        config.pregeneration_config.clone(),
        config.astrometry_config.clone(),
    ) {
        Ok(state) => {
            tracing::info!("✅ Application state initialized successfully");
            state.set_registry_path(config.registry_path.clone());
            state.set_allow_database_management(config.allow_database_management);
            state.set_worker_policy(config.worker_policy);
            tracing::info!(
                "📐 Worker ratios — interactive {:.2}, background {:.2} (of {} logical cores)",
                config.worker_policy.interactive_ratio,
                config.worker_policy.background_ratio,
                crate::concurrency::logical_cores()
            );
            if config.allow_database_management {
                tracing::warn!(
                    "⚠️ Database management via HTTP is ENABLED. Anyone who can reach \
                     this server can add/edit/remove configured databases."
                );
            } else {
                tracing::info!(
                    "🔒 Database management via HTTP is disabled. Pass \
                     --allow-database-management to the server command to enable."
                );
            }
            Arc::new(state)
        }
        Err(e) => {
            tracing::error!("❌ Failed to initialize server: {}", e);
            return Err(e);
        }
    };

    // Kick off a background cache refresh for every configured database.
    for ctx in state.all_databases() {
        let status = ctx.ensure_cache_available();
        match status {
            crate::server::state::RefreshStatus::InProgressWait
            | crate::server::state::RefreshStatus::InProgressServeStale => {
                tracing::info!("🔄 Cache refresh started at server startup (db={})", ctx.id);
            }
            crate::server::state::RefreshStatus::NotNeeded => {
                tracing::info!("✅ Cache is already available at startup (db={})", ctx.id);
            }
            crate::server::state::RefreshStatus::NeedsRefresh => {
                tracing::warn!(
                    "⚠️ Cache refresh needed but not started for db={} - this shouldn't happen",
                    ctx.id
                );
            }
        }
    }

    // Start background image pre-generation if enabled
    if config.pregeneration_config.is_enabled() {
        let state_clone = Arc::clone(&state);
        tokio::spawn(async move {
            background_pregeneration_task(state_clone).await;
        });
    }

    // Per-DB routes — nested under /api/db/{db_id}/.
    let db_routes: Router<Arc<AppState>> = Router::new()
        .route("/refresh-cache", put(handlers::refresh_file_cache))
        .route("/cache-progress", get(handlers::get_cache_refresh_progress))
        .route(
            "/refresh-directory-cache",
            put(handlers::refresh_directory_tree_cache),
        )
        .route("/projects", get(handlers::list_projects))
        .route(
            "/projects/{project_id}",
            put(handlers::update_project_route),
        )
        .route(
            "/projects/{project_id}/merge",
            post(handlers::merge_project_route),
        )
        .route("/targets/{target_id}", put(handlers::update_target_route))
        .route("/projects/overview", get(handlers::get_projects_overview))
        .route("/targets/overview", get(handlers::get_targets_overview))
        .route("/stats/overall", get(handlers::get_overall_stats))
        .route(
            "/projects/{project_id}/targets",
            get(handlers::list_targets),
        )
        .route(
            "/projects/{project_id}/stack-previews",
            post(stack_preview::start_stack_previews),
        )
        .route(
            "/projects/{project_id}/stack-previews/latest",
            get(stack_preview::get_latest_stack_previews),
        )
        .route(
            "/projects/{project_id}/stack-previews/color",
            get(stack_preview::color::get_stack_color_catalog)
                .post(stack_preview::color::start_stack_color),
        )
        .route(
            "/projects/{project_id}/stack-previews/color/{job_id}",
            get(stack_preview::color::get_stack_color_job),
        )
        .route(
            "/projects/{project_id}/stack-previews/{job_id}",
            get(stack_preview::get_stack_preview_job),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/preview",
            get(stack_preview::get_stack_preview_image),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/stretch",
            post(stack_preview::apply_stack_preview_stretch),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/fits",
            get(stack_preview::download_stack_preview_fits),
        )
        .route(
            "/stack-previews/color/{job_id}/preview",
            get(stack_preview::color::get_stack_color_image),
        )
        .route(
            "/stack-previews/stretch/{stretch_id}/preview",
            get(stack_preview::stretch::get_stack_stretch_image),
        )
        .route(
            "/stack-previews/stretch/{stretch_id}/fits",
            get(stack_preview::stretch::download_stack_stretch_fits),
        )
        .route(
            "/stack-previews/color/{job_id}/fits",
            get(stack_preview::color::download_stack_color_fits),
        )
        .route("/images", get(handlers::get_images))
        .route("/images/{image_id}", get(handlers::get_image))
        .route(
            "/images/{image_id}/astrometry",
            get(handlers::get_image_astrometry).post(handlers::solve_image_astrometry),
        )
        .route(
            "/images/{image_id}/satellites",
            get(handlers::get_image_satellites).post(handlers::predict_image_satellites),
        )
        .route(
            "/images/generation-status",
            post(handlers::post_generation_status),
        )
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
        .route("/analysis/sequence", get(handlers::analyze_sequence))
        .route(
            "/analysis/image/{image_id}",
            get(handlers::get_image_quality),
        )
        .route(
            "/analysis/spatial-scan",
            post(handlers::start_spatial_scan).get(handlers::get_spatial_scan_progress),
        )
        .route(
            "/analysis/quality-scan",
            post(handlers::start_spatial_scan).get(handlers::get_spatial_scan_progress),
        )
        .route(
            "/import",
            post(handlers::start_import_route).get(handlers::get_import_progress),
        )
        .route("/export", get(handlers::export_archive_route))
        .route("/export/local", post(handlers::export_local_route));

    // Top-level API: global endpoints + nested per-DB routes.
    let api_routes = Router::new()
        .route("/info", get(handlers::get_server_info))
        .route(
            "/astrometry/capabilities",
            get(handlers::get_astrometry_capabilities),
        )
        .route(
            "/astrometry/catalogs/validate",
            post(handlers::validate_astrometry_catalogs),
        )
        .route(
            "/databases",
            get(handlers::list_databases).post(handlers::add_database_route),
        )
        // Static segment must be declared alongside the {db_id} capture; axum
        // prefers the literal match, so a database slugged "create" can still
        // be updated/deleted (only POST collides, and POST /databases/create
        // is exactly this route).
        .route("/databases/create", post(handlers::create_database_route))
        .route(
            "/databases/{db_id}",
            put(handlers::update_database_route).delete(handlers::remove_database_route),
        )
        .nest("/db/{db_id}", db_routes)
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
    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", config.host, config.port)).await?;

    tracing::info!(
        "🌐 Server listening on http://{}:{}",
        config.host,
        config.port
    );
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
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::task::JoinSet;
    use tokio::time::interval;

    tracing::info!("🎨 Starting background image pre-generation task");

    let scan_interval = Duration::from_secs(300); // Re-scan every 5 minutes
    let mut interval_timer = interval(scan_interval);

    loop {
        interval_timer.tick().await;

        // Iterate every configured database; pre-generation is per-DB work.
        for ctx in state.all_databases() {
            // Yield to interactive work: while a user-triggered scan is
            // running anywhere in the process, skip this cycle entirely so we
            // don't take cores or memory from a job the user is waiting on. We
            // re-check on the next tick.
            if state.interactive_job_active() {
                tracing::debug!(
                    "⏸️ Pre-generation paused (db={}): interactive job running",
                    ctx.id
                );
                continue;
            }

            tracing::debug!(
                "🔍 Scanning db={} for images needing pre-generation",
                ctx.id
            );

            let images = match get_all_images_for_pregeneration(&ctx).await {
                Ok(images) => images,
                Err(e) => {
                    tracing::error!(
                        "❌ Failed to get images for pre-generation (db={}): {}",
                        ctx.id,
                        e
                    );
                    continue;
                }
            };

            if images.is_empty() {
                tracing::debug!("📭 No images found for pre-generation in db={}", ctx.id);
                continue;
            }

            // Background worker budget: fewer cores than interactive work, and
            // it will pause the moment an interactive job starts (below). Probe
            // a representative frame so the same memory ceiling as the scan
            // applies — pre-generation loads full-frame buffers too.
            let frame_pixels = probe_pregen_frame_pixels(&ctx, &images);
            let budget = crate::concurrency::plan_workers(
                None,
                &state.worker_policy(),
                crate::concurrency::Priority::Background,
                frame_pixels,
            );
            let concurrency = budget.workers.max(1);

            tracing::info!(
                "🎯 Pre-generating up to {} images (db={}) with {} background worker(s) — {}",
                images.len(),
                ctx.id,
                concurrency,
                budget.rationale
            );

            // Bound in-flight work to the background budget with a semaphore;
            // each permit is held for one image's whole (multi-format) job.
            let sem = Arc::new(tokio::sync::Semaphore::new(concurrency));
            let mut join_set: JoinSet<(u64, u64, u64)> = JoinSet::new();
            let mut dispatched = 0usize;
            let mut yielded_early = false;

            for (image_id, file_only, target_name) in images {
                // Yield mid-cycle: stop dispatching new work as soon as an
                // interactive job appears; already-running tasks drain.
                if state.interactive_job_active() {
                    yielded_early = true;
                    break;
                }

                let permit = match Arc::clone(&sem).acquire_owned().await {
                    Ok(p) => p,
                    Err(_) => break, // semaphore closed (shouldn't happen)
                };
                let state = Arc::clone(&state);
                let ctx = Arc::clone(&ctx);
                join_set.spawn(async move {
                    let _permit = permit;
                    pregenerate_one_image(&state, &ctx, image_id, &file_only, &target_name).await
                });
                dispatched += 1;
            }

            let (mut generated, mut skipped, mut errors) = (0u64, 0u64, 0u64);
            while let Some(res) = join_set.join_next().await {
                if let Ok((g, s, e)) = res {
                    generated += g;
                    skipped += s;
                    errors += e;
                }
            }

            if dispatched > 0 {
                tracing::info!(
                    "✅ Pre-generation cycle for db={}: {} generated, {} skipped, {} errors ({} images{})",
                    ctx.id,
                    generated,
                    skipped,
                    errors,
                    dispatched,
                    if yielded_early {
                        ", paused early to yield to interactive work"
                    } else {
                        ""
                    }
                );
            }
        }
    }
}

/// Best-effort pixel count of a representative frame from `images`, used to
/// size the background pool's memory ceiling. Resolves basenames through the
/// directory-tree cache (O(1) each, no DB) and probes the first on-disk FITS's
/// `NAXIS` without loading pixels. Scans the whole list until a file resolves,
/// so a leading run of not-on-disk rows (e.g. images for other targets) can't
/// defeat it. `None` (nothing resolvable) falls back to a core-only budget.
fn probe_pregen_frame_pixels(
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    images: &[(i32, String, String)],
) -> Option<usize> {
    let tree = ctx.get_directory_tree().ok()?;
    for (_image_id, file_only, _target_name) in images {
        if let Some(path) = tree.find_file_first(file_only)
            && let Some(px) = crate::concurrency::probe_frame_pixels(path)
        {
            return Some(px);
        }
    }
    None
}

/// Pre-generate every enabled preview format for one image. Returns
/// `(generated, skipped, errors)` counts across the formats.
async fn pregenerate_one_image(
    state: &Arc<AppState>,
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    image_id: i32,
    file_only: &str,
    target_name: &str,
) -> (u64, u64, u64) {
    let (mut generated, mut skipped, mut errors) = (0u64, 0u64, 0u64);

    let mut tally = |result: Result<bool>, what: &str| match result {
        Ok(true) => generated += 1,
        Ok(false) => skipped += 1,
        Err(e) => {
            errors += 1;
            tracing::warn!(
                "⚠️ Failed to pre-generate {} for image {} (db={}): {}",
                what,
                image_id,
                ctx.id,
                e
            );
        }
    };

    if state.pregeneration_config.screen_enabled {
        let r = pregenerate_preview(state, ctx, image_id, file_only, target_name, "screen").await;
        tally(r, "screen preview");
    }
    if state.pregeneration_config.large_enabled {
        let r = pregenerate_preview(state, ctx, image_id, file_only, target_name, "large").await;
        tally(r, "large preview");
    }
    if state.pregeneration_config.original_enabled {
        let r = pregenerate_preview(state, ctx, image_id, file_only, target_name, "original").await;
        tally(r, "original preview");
    }
    if state.pregeneration_config.annotated_enabled {
        let r = pregenerate_annotated(state, ctx, image_id, file_only, target_name).await;
        tally(r, "annotated image");
    }

    (generated, skipped, errors)
}

async fn get_all_images_for_pregeneration(
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
) -> Result<Vec<(i32, String, String)>> {
    // `with_db` reopens and retries if the scheduler DB was replaced out from
    // under our long-lived connection, so this periodic loop self-heals instead
    // of erroring forever. `.context` keeps the underlying rusqlite error in the
    // chain so the corruption detector can see it.
    let images = ctx.with_db(|db| {
        db.query_images(None, None, None, None)
            .context("querying images for pre-generation")
    })?;

    let mut result = Vec::new();

    for (image, _project_name, target_name) in images {
        // Extract filename from metadata
        if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata)
            && let Some(filename_path) = metadata["FileName"].as_str()
        {
            let file_only = filename_path
                .split(&['\\', '/'][..])
                .next_back()
                .unwrap_or(filename_path)
                .to_string();

            result.push((image.id, file_only, target_name));
        }
    }

    Ok(result)
}

async fn pregenerate_preview(
    state: &Arc<AppState>,
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    image_id: i32,
    file_only: &str,
    target_name: &str,
    size: &str,
) -> Result<bool> {
    use crate::server::cache::CacheManager;

    // Get image data from database first (needed for cache key)
    let image_data = {
        use crate::db::Database;

        let conn = ctx.db();
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

    let cache_manager = CacheManager::new(std::path::PathBuf::from(&ctx.cache_dir));
    cache_manager.ensure_category_dir("previews")?;
    let cache_path = cache_manager.get_cached_path("previews", &cache_key, "png");

    // Skip if already cached and not expired
    if cache_manager.is_cached(&cache_path)
        && let Ok(metadata) = tokio::fs::metadata(&cache_path).await
    {
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

    tracing::debug!("🎨 Pre-generating {} preview for image {}", size, image_id);

    // Find FITS file using existing function
    let fits_path = handlers::find_fits_file(ctx, &image_data, target_name, file_only)
        .map_err(|_| anyhow::anyhow!("FITS file not found for image {}", image_id))?;

    // Determine target dimensions
    let max_dimensions = match size {
        "large" => Some((2000, 2000)),
        "screen" => Some((1200, 1200)),
        "original" => None,
        _ => Some((1200, 1200)),
    };

    // Generate atomically via the shared queue helper (temp file then rename),
    // so a concurrent viewer's readiness poll never observes a half-written PNG.
    let job = crate::server::preview_queue::GenJob {
        fits_path,
        cache_path,
        kind: crate::server::preview_queue::GenKind::Preview {
            midtone: 0.2,
            shadow: -2.8,
            max_dimensions,
        },
    };
    tokio::task::spawn_blocking(move || crate::server::preview_queue::generate(&job)).await??;

    tracing::trace!("✅ Generated {} preview for image {}", size, image_id);
    Ok(true) // Successfully generated
}

async fn pregenerate_annotated(
    state: &Arc<AppState>,
    ctx: &Arc<crate::server::database_context::DatabaseContext>,
    image_id: i32,
    file_only: &str,
    target_name: &str,
) -> Result<bool> {
    use crate::server::cache::CacheManager;

    // Get image data from database first (needed for cache key)
    let image_data = {
        use crate::db::Database;

        let conn = ctx.db();
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

    let cache_manager = CacheManager::new(std::path::PathBuf::from(&ctx.cache_dir));
    cache_manager.ensure_category_dir("annotated")?;
    let cache_path = cache_manager.get_cached_path("annotated", &cache_key, "png");

    // Skip if already cached and not expired
    if cache_manager.is_cached(&cache_path)
        && let Ok(metadata) = tokio::fs::metadata(&cache_path).await
    {
        let age = metadata.modified()?.elapsed().unwrap_or_default();
        if age < state.pregeneration_config.cache_expiry {
            tracing::trace!(
                "⏭️ Skipping annotated pre-generation for image {}: already cached",
                image_id
            );
            return Ok(false); // Skipped, not generated
        }
    }

    tracing::debug!("🎨 Pre-generating annotated image for image {}", image_id);

    // Find FITS file
    let fits_path = handlers::find_fits_file(ctx, &image_data, target_name, file_only)
        .map_err(|_| anyhow::anyhow!("FITS file not found for image {}", image_id))?;

    // Generate atomically via the shared queue helper — consistent sizing with
    // the on-demand path, and temp-then-rename so a viewer never sees a partial
    // file (the old direct File::create write was NOT atomic).
    let job = crate::server::preview_queue::GenJob {
        fits_path,
        cache_path,
        kind: crate::server::preview_queue::GenKind::Annotated {
            max_stars: max_stars as usize,
            size: size.to_string(),
        },
    };
    tokio::task::spawn_blocking(move || crate::server::preview_queue::generate(&job)).await??;

    tracing::trace!("✅ Generated annotated image for image {}", image_id);
    Ok(true) // Successfully generated
}
