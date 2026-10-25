pub mod api;
pub mod astrobin_export;
pub mod auth;
pub mod autoimport;
pub mod backup_job;
pub mod cache;
pub mod cache_budget;
pub mod calibration_browser;
pub mod calibration_duplicates;
pub mod calibration_health;
pub mod calibration_quality;
pub mod calibration_scope;
pub mod calibration_settings;
pub mod catalog_install;
pub mod catalog_jobs;
pub mod catalog_refresh;
pub mod catalog_upkeep;
pub mod database_context;
pub mod director;
pub mod embedded_static;
pub mod export_job;
pub mod export_settings;
pub mod exposure_groups;
pub mod extract;
pub mod file_body;
pub mod flat_choices;
pub mod flat_group_use;
pub mod flat_groups;
pub mod flat_history;
pub mod flat_measure;
pub mod frame_shapes;
pub mod frame_sites;
pub mod handlers;
pub mod import_job;
pub mod job_line;
pub mod job_text;
pub mod jobs;
pub(crate) mod listener;
pub mod master_cleanup;
pub mod mcp;
pub mod mosaic_scope;
pub mod organization;
pub mod page_build;
pub mod pairing;
pub mod peers;
pub mod preview_pregeneration;
pub mod preview_queue;
pub mod previews;
pub mod processing_setups;
pub mod project_removal;
pub mod quality_arrival;
pub mod quality_backfill;
pub mod quality_scans;
pub mod reject_removal;
pub mod remote_audit;
pub mod remote_sync;
pub mod remote_upload;
pub mod remote_upload_layout;
pub mod schedule;
pub mod scheduler;
pub mod scoring_defaults;
pub mod shutdown;
pub mod sky_coverage;
pub mod slug;
pub mod spatial_scan;
pub mod stack_preview;
pub mod stack_settings;
pub mod state;
pub mod static_file_service;
pub mod storage;
pub mod storage_settings;
pub mod sync_preview;
pub mod ts_sync;
pub mod update_notice;
pub mod user_admin;
pub mod user_preferences;
pub mod wbpp_queue;
pub mod wbpp_run;
pub mod wbpp_storage;
pub mod workers;

use axum::{
    extract::DefaultBodyLimit,
    routing::{delete, get, post, put},
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};

use crate::server::embedded_static::serve_embedded_file;
use crate::server::page_build::PageBuild;
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
    /// Storage folders the config file or command line fixes, and the
    /// default cache. Settings choose the rest.
    pub storage: storage::StorageConfig,
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
    /// Director coordination store, separate from every catalog. `None` means
    /// the default file beside `registry_path` when database management is on.
    pub director_meta: Option<PathBuf>,
    /// Trust every session-less caller even on a routable bind address, so a
    /// server with no user accounts stays open the way a localhost server is.
    /// Off by default. Loopback binds are trusted without it.
    pub allow_anonymous_access: bool,
    /// Optional notice shown below the application header.
    pub site_banner: Option<crate::config::SiteBannerConfig>,
    /// Optional browser/server authentication. Tauri always leaves this off.
    pub auth: Option<auth::ServerAuth>,
    /// Tuning policy for the parallel scans and background pre-generation.
    /// See `concurrency::WorkerPolicy`.
    pub worker_policy: crate::concurrency::WorkerPolicy,
    /// How generated previews are encoded. PNG unless the TOML `[server]`
    /// section asks for JPEG.
    pub preview_encoding: crate::preview_format::PreviewEncoding,
    /// Whether previews default to colour.
    pub preview_color_default: bool,
    /// Keep a failed remote upload's staging file for inspection.
    pub keep_failed_uploads: bool,
    /// Process-global Seiza catalog configuration from the shared registry.
    pub astrometry_config: Option<crate::astrometry::AstrometryConfig>,
    /// The server's stop, when the caller wants to see what a stop waits
    /// for: the desktop app does, to say why a restart is taking long.
    /// `None` makes a new one.
    pub shutdown: Option<Arc<shutdown::Shutdown>>,
    /// The TOML file the server started with, which a backup copies.
    pub config_path: Option<PathBuf>,
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

    run_server_internal(config, None, None, None).await
}

/// Set once the first server of this process starts.
static SERVER_STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Apply the process-wide settings the registry holds. The one place that
/// does, so the server and the desktop app start the same way and a restart
/// picks up what Settings saved. Runs before any catalog opens.
fn apply_registry_settings(registry: &crate::db_registry::DbRegistry) {
    crate::calibration::configure(registry.calibration.as_ref());
    stack_preview::automatic::configure_from_registry(registry.stacking.as_ref());
    scoring_defaults::configure(registry.scoring.as_ref());
    cache_budget::configure(registry.storage.as_ref());
}

/// Whether a caller with no session is trusted while the server has no user
/// accounts. Reaching a loopback server already means reaching the machine,
/// so it stays open. Any other address needs the operator to say so.
fn anonymous_access_is_trusted(host: &str, allow_anonymous_access: bool) -> bool {
    auth::host_is_loopback(host) || allow_anonymous_access
}

/// Refuse a network server that would hand out database management with no
/// login. Loopback servers — the desktop app, and local development — keep
/// working, as does a server whose operator asked for anonymous access.
fn check_management_needs_a_login(
    host: &str,
    allow_database_management: bool,
    anonymous_access_trusted: bool,
    has_users: bool,
) -> anyhow::Result<()> {
    if allow_database_management && !has_users && !anonymous_access_trusted {
        anyhow::bail!(
            "Database management on {host} needs a login. Add an editor with \
             `psf-guard users add <name> --role read-write` (pass the same \
             --registry you give the server), or bind the server to 127.0.0.1. \
             --allow-anonymous-access serves it to everyone instead."
        );
    }
    Ok(())
}

/// The `/api` router: every route, the MCP endpoint, and the JSON and
/// authorization layers.
pub fn api_router(state: Arc<AppState>) -> Router {
    let db_routes: Router<Arc<AppState>> = Router::new()
        .route("/refresh-cache", put(handlers::refresh_file_cache))
        .route("/cache-progress", get(handlers::get_cache_refresh_progress))
        .route(
            "/refresh-directory-cache",
            put(handlers::refresh_directory_tree_cache),
        )
        .route("/projects", get(handlers::list_projects))
        .route(
            "/projects/{project_id}/processing-settings",
            get(exposure_groups::get_project_settings)
                .put(exposure_groups::update_project_settings),
        )
        .route("/rejects/removal", get(reject_removal::get_status))
        .route("/rejects/removal/stop", post(reject_removal::stop_removal))
        .route("/rejects/removal/preview", post(reject_removal::preview))
        .route("/rejects/removal/apply", post(reject_removal::apply))
        .route("/rejects/removed", get(reject_removal::removed))
        .route("/rejects/removed/restore", post(reject_removal::restore))
        .route("/rejects/trash/empty", post(reject_removal::empty_trash))
        .route("/rejects/removed/purge", post(reject_removal::purge))
        .route("/organization/preview", post(organization::preview))
        .route("/organization/apply", post(organization::apply))
        .route(
            "/organization/destinations",
            get(organization::destinations),
        )
        .route("/calibrations", get(handlers::get_calibration_library))
        .route("/flat-history", get(flat_history::list))
        .route("/flat-history/invalidate", post(flat_history::invalidate))
        .route(
            "/calibrations/details",
            get(handlers::get_calibration_library_details),
        )
        .route(
            "/calibrations/frames/{frame_uuid}",
            get(calibration_browser::get_frame).delete(handlers::forget_calibration_frame),
        )
        .route(
            "/calibrations/frames/{frame_uuid}/preview",
            get(calibration_browser::get_frame_preview),
        )
        .route(
            "/calibrations/sessions",
            get(calibration_browser::list_sessions),
        )
        .route(
            "/calibrations/duplicates",
            get(calibration_duplicates::get_duplicates),
        )
        .route(
            "/calibrations/duplicates/check",
            post(calibration_duplicates::check_duplicates),
        )
        .route(
            "/calibrations/duplicates/repair",
            post(calibration_duplicates::repair_duplicates),
        )
        .route(
            "/calibrations/duplicates/stop",
            post(calibration_duplicates::stop_duplicates),
        )
        .route(
            "/calibrations/generation-status",
            post(calibration_browser::post_generation_status),
        )
        .route(
            "/calibrations/frames/validity",
            put(handlers::set_calibration_validity),
        )
        .route(
            "/calibrations/frames/quality",
            put(handlers::set_calibration_quality_override),
        )
        .route(
            "/calibrations/quality",
            get(calibration_quality::get_progress),
        )
        .route(
            "/calibrations/project-scope",
            get(calibration_scope::get_project_scope),
        )
        .route(
            "/calibrations/flat-groups",
            get(flat_groups::get_flat_groups),
        )
        .route("/calibrations/flat-measure", get(flat_measure::get_status))
        .route(
            "/calibrations/flat-measure/watch",
            post(flat_measure::post_watch),
        )
        .route("/calibrations/frame-sites", get(frame_sites::get_status))
        .route("/jobs/paused", get(catalog_jobs::get_paused))
        .route("/jobs/{job}/pause", post(catalog_jobs::post_pause))
        .route("/jobs/{job}/resume", post(catalog_jobs::post_resume))
        .route(
            "/calibrations/flat-groups/ratio",
            get(flat_groups::get_ratio_picture),
        )
        .route(
            "/calibrations/flat-groups/use",
            get(flat_group_use::get_flat_group_use),
        )
        .route(
            "/calibrations/flat-groups/place",
            get(flat_groups::get_session_place),
        )
        .route(
            "/calibrations/flat-groups/overrides",
            put(flat_choices::put_group_override),
        )
        .route(
            "/calibrations/flat-choices",
            put(flat_choices::put_light_choices),
        )
        .route(
            "/calibrations/flat-choices/options",
            post(flat_choices::post_light_choice_options),
        )
        .route(
            "/calibrations/frames",
            get(calibration_browser::list_frames).delete(handlers::forget_calibration_frames),
        )
        .route(
            "/calibrations/masters",
            get(calibration_browser::list_masters).delete(handlers::clear_calibration_masters),
        )
        .route(
            "/calibrations/masters/{master_uuid}/preview",
            get(calibration_browser::get_master_preview),
        )
        .route(
            "/calibrations/masters/{master_uuid}/fits",
            get(calibration_browser::download_master_fits),
        )
        .route(
            "/projects/{project_id}",
            put(handlers::update_project_route),
        )
        .route("/guids", get(handlers::missing_guids_route))
        .route("/guids/fill", post(handlers::fill_guids_route))
        .route(
            "/projects/{project_id}/merge",
            post(handlers::merge_project_route),
        )
        .route(
            "/projects/{project_id}/removal",
            get(project_removal::preview).post(project_removal::remove),
        )
        .route("/deleted-projects", get(project_removal::deleted))
        .route(
            "/deleted-projects/{id}/allow",
            post(project_removal::allow_back),
        )
        .route(
            "/projects/{project_id}/scheduler",
            get(scheduler::get_project_scheduler),
        )
        .route("/targets/{target_id}", put(handlers::update_target_route))
        .route(
            "/targets/{target_id}/exposure-plans",
            post(scheduler::create_exposure_plan),
        )
        .route(
            "/exposure-plans/{plan_id}",
            put(scheduler::update_exposure_plan),
        )
        .route("/projects/overview", get(handlers::get_projects_overview))
        .route("/targets", get(handlers::list_all_targets))
        .route("/targets/overview", get(handlers::get_targets_overview))
        .route("/stats/overall", get(handlers::get_overall_stats))
        .route("/sky/coverage", get(sky_coverage::get_sky_coverage))
        .route(
            "/projects/{project_id}/calibration-report",
            get(handlers::get_project_calibration_report),
        )
        .route(
            "/projects/{project_id}/calibration-health",
            get(calibration_health::get_project_health),
        )
        .route("/calibration-health", get(calibration_health::get_health))
        .route(
            "/calibration-health/lights",
            get(calibration_health::get_light_marks),
        )
        .route(
            "/projects/{project_id}/calibration-report/rejects",
            get(handlers::get_project_calibration_gaps).post(handlers::reject_uncalibrated_lights),
        )
        .route(
            "/projects/{project_id}/targets",
            get(handlers::list_targets),
        )
        .route(
            "/projects/{project_id}/mosaic",
            get(mosaic_scope::get_project_mosaic),
        )
        .route(
            "/projects/{project_id}/stack-previews",
            post(stack_preview::start_stack_previews),
        )
        .route(
            "/projects/{project_id}/stack-previews/channels",
            post(stack_preview::start_stack_preview_channels),
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
            "/projects/{project_id}/stack-previews/wbpp",
            get(stack_preview::get_wbpp_stacks),
        )
        .route(
            "/projects/{project_id}/stack-previews/wbpp/import",
            post(stack_preview::import_wbpp_stacks),
        )
        .route(
            "/projects/{project_id}/stack-previews/{job_id}/cancel",
            post(stack_preview::cancel_stack_preview_job),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/preview",
            get(stack_preview::get_stack_preview_image),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/stretch",
            post(stack_preview::apply_stack_preview_stretch)
                .get(stack_preview::get_stack_preview_processing)
                .delete(stack_preview::clear_stack_preview_processing),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/artifact-searches",
            post(stack_preview::artifact::start_mono_artifact_search),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/calibration-masters",
            get(stack_preview::calibration_masters::get_catalog),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/calibration-masters/{master_id}/preview",
            get(stack_preview::calibration_masters::get_preview),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/calibration-masters/{master_id}/fits",
            get(stack_preview::calibration_masters::download_fits),
        )
        .route(
            "/stack-previews/color/{job_id}/calibration-masters",
            get(stack_preview::calibration_masters::get_catalog),
        )
        .route(
            "/stack-previews/color/{job_id}/calibration-masters/{master_id}/preview",
            get(stack_preview::calibration_masters::get_preview),
        )
        .route(
            "/stack-previews/color/{job_id}/calibration-masters/{master_id}/fits",
            get(stack_preview::calibration_masters::download_fits),
        )
        .route(
            "/stack-previews/calibration-masters/generation-status",
            post(stack_preview::calibration_masters::post_generation_status),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/fits",
            get(stack_preview::download_stack_preview_fits),
        )
        .route(
            "/stack-previews/{job_id}/{group_index}/snr",
            get(stack_preview::get_stack_preview_snr),
        )
        .route(
            "/stack-previews/color/{job_id}/preview",
            get(stack_preview::color::get_stack_color_image),
        )
        .route(
            "/stack-previews/color/{job_id}/artifact-searches",
            post(stack_preview::artifact::start_color_artifact_search),
        )
        .route(
            "/stack-previews/artifact-searches/{search_id}",
            get(stack_preview::artifact::get_artifact_search),
        )
        .route(
            "/stack-previews/artifact-searches/{search_id}/crops/{image_id}",
            get(stack_preview::artifact::get_artifact_crop),
        )
        .route(
            "/stack-previews/rc-astro/{rc_astro_id}/fits",
            get(stack_preview::rc_astro::download_rc_astro_fits),
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
            "/stack-previews/stretch/{stretch_id}/stars",
            get(stack_preview::stretch::get_stack_stretch_stars_image),
        )
        .route(
            "/stack-previews/stretch/{stretch_id}/stars-fits",
            get(stack_preview::stretch::download_stack_stretch_stars_fits),
        )
        .route(
            "/stack-previews/color/{job_id}/continuum/{role}/{kind}",
            get(stack_preview::color::get_stack_color_continuum_artifact),
        )
        .route(
            "/stack-previews/color/{job_id}/fits",
            get(stack_preview::color::download_stack_color_fits),
        )
        .route("/images", get(handlers::get_images))
        .route(
            "/images/upload",
            post(remote_upload::upload_image)
                .layer(DefaultBodyLimit::max(remote_upload::MAX_MULTIPART_BYTES)),
        )
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
            "/images/{image_id}/calibration",
            get(handlers::get_image_calibration),
        )
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
        .route("/images/grade", post(handlers::batch_update_image_grades))
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
            "/analysis/quality-backfill",
            post(handlers::start_quality_backfill_route)
                .get(handlers::get_quality_backfill_progress),
        )
        .route(
            "/analysis/quality-backfill/new-frames",
            axum::routing::put(handlers::update_analyze_new_frames),
        )
        .route(
            "/import",
            post(handlers::start_import_route).get(handlers::get_import_progress),
        )
        .route("/import/stop", post(handlers::stop_import_route))
        .route(
            "/calibrated-copies",
            get(handlers::get_calibrated_copies).put(handlers::update_calibrated_copies),
        )
        .route("/autoimport", get(handlers::get_autoimport_status))
        .route("/autoimport/run", post(handlers::run_autoimport_now))
        .route("/import/folders", get(handlers::get_import_folders))
        .route("/export", get(handlers::export_archive_route))
        .route(
            "/astrobin-export",
            get(astrobin_export::get_astrobin_export),
        )
        .route(
            "/astrobin-export.csv",
            get(astrobin_export::get_astrobin_csv),
        )
        .route(
            "/astrobin/filters",
            get(astrobin_export::get_astrobin_filters)
                .put(astrobin_export::update_astrobin_filters),
        )
        .route("/wbpp/runs", post(wbpp_run::start_wbpp_run))
        .route(
            "/wbpp/runs/current",
            get(wbpp_run::get_wbpp_run).delete(wbpp_run::cancel_wbpp_run),
        )
        .route(
            "/wbpp/runs/current/publish",
            post(wbpp_run::publish_wbpp_run),
        )
        .route(
            "/wbpp/runs/current/dismiss",
            post(wbpp_run::dismiss_wbpp_run),
        )
        .route(
            "/wbpp/runs/queue/{queue_id}",
            delete(wbpp_run::remove_queued_wbpp_run),
        )
        .route(
            "/wbpp/runs/current/files/{*path}",
            get(wbpp_run::get_wbpp_run_file),
        )
        .route("/export/local", post(handlers::export_local_route))
        .route(
            "/export/server",
            post(handlers::start_server_export_route).get(handlers::get_server_export_progress),
        )
        .route(
            "/export/server/stop",
            post(handlers::stop_server_export_route),
        );

    // Top-level API: global endpoints + nested per-DB routes.
    let api_routes = Router::new()
        .route("/auth/status", get(auth::status))
        .route("/auth/login", post(auth::login))
        .route("/auth/logout", post(auth::logout))
        .route(
            "/auth/users",
            get(user_admin::list_users).post(user_admin::create_user),
        )
        .route(
            "/auth/users/{username}",
            put(user_admin::update_user).delete(user_admin::remove_user),
        )
        .route(
            "/auth/tokens",
            get(user_admin::list_tokens).post(user_admin::create_token),
        )
        .route("/auth/tokens/{id}", delete(user_admin::revoke_token))
        .route(
            "/me/preferences",
            get(user_preferences::get_preferences)
                .patch(user_preferences::patch_preferences)
                .put(user_preferences::put_preferences)
                .layer(DefaultBodyLimit::max(
                    crate::user_preferences::MAX_REQUEST_BYTES,
                )),
        )
        .route("/info", get(handlers::get_server_info))
        .route("/jobs", get(jobs::get_jobs))
        .route("/stack-activity", get(stack_preview::get_stack_activity))
        .route(
            "/stack-activity/scheduled/skip",
            post(stack_preview::skip_scheduled_refresh),
        )
        .route(
            "/stack-activity/scheduled/run-now",
            post(stack_preview::run_scheduled_refresh_now),
        )
        .route(
            "/stack-activity/{job_id}/move",
            post(stack_preview::move_stack_job),
        )
        .route(
            "/stack-activity/{job_id}/cancel",
            post(stack_preview::cancel_stack_activity_job),
        )
        .route("/wbpp/activity", get(wbpp_run::get_wbpp_activity))
        .route(
            "/wbpp/queue/{queue_id}/move",
            post(wbpp_run::move_queued_wbpp_run),
        )
        .route(
            "/settings/calibration",
            get(calibration_settings::get_calibration_settings)
                .put(calibration_settings::update_calibration_settings),
        )
        .route(
            "/settings/calibration/rigs/{rig_uuid}",
            put(calibration_settings::update_rig_calibration_settings)
                .delete(calibration_settings::delete_rig_calibration_settings),
        )
        .route(
            "/settings/export",
            get(export_settings::get_export_settings).put(export_settings::update_export_settings),
        )
        .route(
            "/settings/astrobin",
            get(astrobin_export::get_astrobin_settings)
                .put(astrobin_export::update_astrobin_settings),
        )
        .route(
            "/settings/pixinsight",
            get(wbpp_run::get_pixinsight_settings).put(wbpp_run::update_pixinsight_settings),
        )
        .route(
            "/settings/stacking",
            get(stack_settings::get_stack_settings).put(stack_settings::update_stack_settings),
        )
        .route(
            "/settings/workers",
            get(stack_settings::get_worker_settings).put(stack_settings::update_worker_settings),
        )
        .route(
            "/settings/storage",
            get(storage_settings::get_storage_settings)
                .put(storage_settings::update_storage_settings),
        )
        .route(
            "/settings/storage/folders",
            axum::routing::put(storage_settings::update_storage_folders),
        )
        .route(
            "/settings/scoring",
            get(scoring_defaults::get_scoring_defaults)
                .put(scoring_defaults::update_scoring_defaults),
        )
        .route(
            "/settings/stacking/method",
            get(stack_settings::get_stack_method).put(stack_settings::update_stack_method),
        )
        .route(
            "/settings/stacking/gpu",
            get(stack_settings::get_stack_gpu).put(stack_settings::update_stack_gpu),
        )
        .route(
            "/processing-setups",
            get(processing_setups::list_setups).post(processing_setups::save_setup),
        )
        .route(
            "/processing-setups/import",
            post(processing_setups::import_setups),
        )
        .route(
            "/processing-setups/{name}",
            axum::routing::delete(processing_setups::delete_setup),
        )
        .route(
            "/processing-setups/{name}/default",
            axum::routing::put(processing_setups::mark_default_setup),
        )
        .route("/update-notice", get(handlers::get_update_notice))
        .nest("/director/v1", director::routes())
        .route(
            "/astrometry/capabilities",
            get(handlers::get_astrometry_capabilities),
        )
        .route(
            "/tools/rc-astro",
            get(stack_preview::rc_astro::get_rc_astro_capabilities),
        )
        .route(
            "/astrometry/catalogs/validate",
            post(handlers::validate_astrometry_catalogs),
        )
        .route(
            "/astrometry/catalogs/install",
            get(handlers::get_astrometry_catalog_install)
                .post(handlers::start_astrometry_catalog_install),
        )
        .route(
            "/astrometry/catalogs/install/stop",
            post(handlers::stop_astrometry_catalog_install),
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
        .route("/databases/import-ts", post(ts_sync::import_route))
        .route("/ts-sync", get(ts_sync::status_route))
        .route("/ts-sync/{job_id}/stop", post(ts_sync::stop_route))
        .route("/previews/{kind}/{key}/stop", post(previews::stop_route))
        .route(
            "/databases/{db_id}/sync",
            post(handlers::sync_database_route),
        )
        .route(
            "/databases/{db_id}/sync/preview",
            post(handlers::preview_sync_database_route),
        )
        .route(
            "/databases/{db_id}/sync/previews",
            get(handlers::list_sync_database_previews_route),
        )
        .route(
            "/databases/{db_id}/pairing-token",
            post(pairing::issue_pairing_token_route),
        )
        .route(
            "/databases/{db_id}/clients/{client_uuid}",
            delete(pairing::revoke_client_route),
        )
        .route(
            "/databases/{db_id}/sync/previews/{preview_id}/apply",
            post(handlers::apply_sync_database_preview_route),
        )
        .route(
            "/databases/{db_id}/sync/previews/{preview_id}/refresh",
            post(handlers::refresh_sync_database_preview_route),
        )
        .route(
            "/databases/{db_id}/sync/previews/{preview_id}",
            get(handlers::get_sync_database_preview_route)
                .delete(handlers::delete_sync_database_preview_route),
        )
        .route(
            "/databases/{db_id}",
            put(handlers::update_database_route).delete(handlers::remove_database_route),
        )
        .route("/peers", get(peers::list_peers).post(peers::add_peer))
        .route(
            "/peers/{peer_id}",
            put(peers::update_peer).delete(peers::remove_peer),
        )
        .route("/peers/{peer_id}/check", post(peers::check_peer))
        .route(
            "/databases/{db_id}/sync/remote",
            post(peers::sync_with_peer),
        )
        .route("/sync/v1/capabilities", get(remote_sync::capabilities))
        .route(
            "/sync/v1/flat-history/snapshot",
            post(flat_history::snapshot).layer(DefaultBodyLimit::max(flat_history::MAX_BODY_BYTES)),
        )
        .route(
            "/sync/v1/flat-history/pending",
            post(flat_history::pending).layer(DefaultBodyLimit::max(flat_history::MAX_BODY_BYTES)),
        )
        .route(
            "/sync/v1/flat-history/acknowledge",
            post(flat_history::acknowledge)
                .layer(DefaultBodyLimit::max(flat_history::MAX_BODY_BYTES)),
        )
        .route("/sync/v1/pair", post(pairing::pair_route))
        .route(
            "/sync/v1/previews",
            post(remote_sync::create_preview)
                .layer(DefaultBodyLimit::max(remote_sync::MAX_SYNC_BODY_BYTES)),
        )
        .route(
            "/sync/v1/previews/{preview_id}",
            get(remote_sync::get_preview),
        )
        .route(
            "/sync/v1/previews/{preview_id}/apply",
            post(remote_sync::apply_preview),
        )
        .route(
            "/sync/v1/previews/{preview_id}/refresh",
            post(remote_sync::refresh_preview),
        )
        .route("/sync/v1/exports", post(remote_sync::create_export))
        .route("/sync/v1/exports/{export_id}", get(remote_sync::get_export))
        .route("/sync/v1/jobs/{job_id}", get(remote_sync::get_preview_job))
        .merge(backup_job::routes())
        .nest("/workers/v1", workers::routes::worker_routes())
        .merge(workers::routes::settings_routes())
        .nest("/db/{db_id}", db_routes)
        .nest_service("/mcp", mcp::service(Arc::clone(&state)))
        .layer(axum::middleware::from_fn(json_no_store))
        .layer(axum::middleware::from_fn_with_state(
            Arc::clone(&state),
            auth::authorize_api,
        ))
        .with_state(Arc::clone(&state));
    // The MCP tools send their requests through this same router, as the
    // caller, so every gate the UI meets applies to an agent too.
    state.set_api_router(api_routes.clone());
    if let Ok(runtime) = tokio::runtime::Handle::try_current() {
        state.set_runtime(runtime);
    }
    api_routes
}

async fn run_server_internal(
    config: ServerConfig,
    shutdown_rx: Option<oneshot::Receiver<()>>,
    listener: Option<listener::SharedTcpListener>,
    ready: Option<oneshot::Sender<()>>,
) -> anyhow::Result<()> {
    let anonymous_access_trusted =
        anonymous_access_is_trusted(&config.host, config.allow_anonymous_access);
    check_management_needs_a_login(
        &config.host,
        config.allow_database_management,
        anonymous_access_trusted,
        config.auth.is_some(),
    )?;

    let director_meta = director::resolve_meta_path(
        config.director_meta.as_deref(),
        config.registry_path.as_deref(),
    );
    director::validate_registry_separation(
        director_meta.as_deref(),
        config.registry_path.as_deref(),
    )?;
    let director = director::Service::configured_or_off(director_meta.as_deref());
    if let (Some(path), Some(_)) = (&director_meta, &director) {
        tracing::info!("🧭 Director meta store: {}", path.display());
    }

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

    // Settle the storage folders, moving files when a setting changed one,
    // before any database opens a folder below them.
    let storage_config = config.storage.clone();
    let registry_for_storage = config.registry_path.clone();
    // Only the first server of a process moves files: one restarted inside
    // the desktop app may still have the old one's background work writing.
    let allow_moves = !SERVER_STARTED.swap(true, std::sync::atomic::Ordering::SeqCst);
    let startup_storage = tokio::task::spawn_blocking(move || {
        storage::prepare(
            &storage_config,
            registry_for_storage.as_deref(),
            allow_moves,
        )
    })
    .await?;
    for note in &startup_storage.notes {
        if note.starts_with("Moved") {
            tracing::info!("📦 {note}");
        } else {
            tracing::warn!("📦 {note}");
        }
    }
    tracing::info!(
        "💾 Cache directory: {}",
        startup_storage.roots.cache.display()
    );
    tracing::info!(
        "💾 Stack directory: {}",
        startup_storage.roots.stacks.display()
    );
    tracing::info!(
        "💾 Calibration master directory: {}",
        startup_storage.roots.calibration.display()
    );
    // Read after the storage step, which may have recorded the folders.
    let registry = config
        .registry_path
        .as_deref()
        .and_then(|path| crate::db_registry::DbRegistry::load_or_init(path).ok());
    if let Some(registry) = &registry {
        apply_registry_settings(registry);
    }

    // Create app state
    let state = match AppState::from_databases_with_astrometry(
        config.databases.clone(),
        startup_storage.roots.clone(),
        config.pregeneration_config.clone(),
        config.astrometry_config.clone(),
    ) {
        Ok(mut state) => {
            state.director = director;
            if let Some(shutdown) = &config.shutdown {
                state.shutdown = Arc::clone(shutdown);
            }
            tracing::info!("✅ Application state initialized successfully");
            state.set_registry_path(config.registry_path.clone());
            state
                .backups
                .set_locations(config.config_path.clone(), director_meta.clone());
            state.set_allow_database_management(config.allow_database_management);
            state.set_site_banner(config.site_banner.clone());
            state.set_server_auth(config.auth.clone());
            if let (Some(auth), Some(registry_path)) = (&config.auth, &config.registry_path) {
                user_preferences::forget_removed_users(auth, registry_path);
            }
            state.set_anonymous_access_trusted(anonymous_access_trusted);
            state.set_worker_policy(config.worker_policy);
            state.set_preview_encoding(config.preview_encoding);
            state.set_preview_color_default(config.preview_color_default);
            state.workers.configure(
                registry
                    .as_ref()
                    .and_then(|registry| registry.remote_workers.clone())
                    .unwrap_or_default(),
                startup_storage.roots.cache.join("workers"),
            );
            remote_upload::configure_keep_failed_uploads(config.keep_failed_uploads);
            if config.keep_failed_uploads {
                tracing::info!(
                    "🧪 keep_failed_uploads is on: a failed remote upload leaves its staged file \
                     in the receive directory for inspection"
                );
            }
            if let Some(banner) = &config.site_banner {
                tracing::info!("📢 Site banner enabled: {}", banner.title);
            }
            if config.auth.is_some() {
                tracing::info!("🔐 Browser authentication enabled");
            } else if !anonymous_access_trusted {
                tracing::warn!(
                    "🔐 No user accounts, so {} answers 401 to every API request. Add one \
                     with `psf-guard users add <name> --role read-write` and restart, or \
                     bind the server to 127.0.0.1 to keep it on this machine. Remote sync \
                     and image upload keys keep working either way.",
                    config.host
                );
            } else if config.allow_anonymous_access && !auth::host_is_loopback(&config.host) {
                tracing::warn!(
                    "⚠️ --allow-anonymous-access is ON with no user accounts. Everyone who \
                     can reach {} gets full editor access to every catalog. Add users and \
                     drop the flag as soon as you can.",
                    config.host
                );
            } else {
                tracing::info!(
                    "🔓 No user accounts; {} is a loopback server and stays open to this machine",
                    config.host
                );
            }
            state.set_storage_status(state::StorageStatus {
                config: config.storage.clone(),
                notes: startup_storage.notes.clone(),
            });
            for (slug, from, to) in &startup_storage.calibration_moves {
                if let Some(ctx) = state.get_database(slug) {
                    storage::relocate::follow_master_rows(&ctx, from, to);
                }
            }
            // Shares chosen in Settings sit over the config file's.
            if let Some(registry) = &registry {
                state.apply_worker_settings(registry.workers.as_ref());
            }
            let policy = state.worker_policy();
            tracing::info!(
                "📐 Worker ratios — interactive {:.2}, background {:.2} (of {} logical cores)",
                policy.interactive_ratio,
                policy.background_ratio,
                crate::concurrency::logical_cores()
            );
            report_preview_settings(&state, &config);
            if config.allow_database_management {
                if config.auth.is_some() {
                    tracing::warn!(
                        "⚠️ Database management via HTTP is enabled for authenticated editors."
                    );
                } else {
                    tracing::warn!(
                        "⚠️ Database management via HTTP is ENABLED with no login. Anyone \
                         who can reach {} can add/edit/remove configured databases.",
                        config.host
                    );
                }
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

    // Release feeds are process-global. Refresh once now and then every 24
    // hours; browser reloads only read this cache through the API.
    state.update_notices.start_refresh_loop(&state);
    crate::server::wbpp_storage::spawn(&state);

    // For the job journal's last write at shutdown; the router takes `state`.
    let journal_state = Arc::clone(&state);

    // Remembered stack previews follow the catalog when the operator asked
    // for that; the scheduler idles otherwise.
    crate::server::stack_preview::automatic::spawn(Arc::clone(&state));

    // The queue from before a restart comes back, then the journal keeps it
    // from here on. Frames that arrived while the server was down reach
    // remembered previews through one check of every database, after the
    // usual settling delay; a check that finds nothing new starts nothing.
    {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            crate::server::stack_preview::journal::restore(&state).await;
            crate::server::stack_preview::journal::spawn_writer(Arc::clone(&state));
            if crate::server::stack_preview::automatic::policy().enabled {
                for ctx in state.all_databases() {
                    state.auto_stacks.touch_database(
                        &ctx.id,
                        crate::server::stack_preview::automatic::RefreshReason::Sync,
                    );
                }
            }
        });
    }

    // Databases that asked for it import new frames on open and on schedule.
    crate::server::autoimport::spawn(Arc::clone(&state));
    crate::server::ts_sync::spawn(Arc::clone(&state));
    // Periodic backups, when Settings turned them on.
    crate::server::backup_job::spawn(Arc::clone(&state));
    crate::server::director::spawn_collaboration(Arc::clone(&state));

    // Each catalog's upkeep (query indexes, then readout modes) on its
    // runner, off the request path and before cache refreshes start
    // long-lived reads. A database added or changed later asks again
    // (`AppState::add_database`). Each step uses a short busy timeout, so a
    // writer holding the catalog wins and the step waits for the next
    // request.
    {
        let paths: Vec<String> = state
            .databases
            .read()
            .map(|databases| {
                databases
                    .values()
                    .map(|ctx| ctx.database_path.clone())
                    .collect()
            })
            .unwrap_or_default();
        for path in paths {
            crate::server::catalog_upkeep::start(&state, &path);
            // Frames imported before the catalog kept their sites: a rig
            // whose sites it reads is graded again and its flats planned
            // again as each chunk commits.
            crate::server::frame_sites::queue(&state, &path);
            crate::server::calibration_quality::request(&state, &path);
            // Only what is unfinished or stale: a catalog that is up to date
            // reads no pixels.
            crate::server::flat_measure::queue(
                &state,
                &path,
                crate::calibration::flat_groups::job::Scope::everything(),
            );
        }
    }

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

    // Keep the cache's volume under its limit, culling previews first.
    cache_budget::spawn(&state);
    // Remove superseded stacks in every database, not only those building.
    stack_preview::spawn_janitor(&state);
    // Scan newly arrived frames where a database asks for it.
    quality_arrival::note_start(&state);
    frame_sites::spawn_away_watch(&state);

    // Start preview pre-generation when the server was started with it on.
    preview_pregeneration::spawn(&state);
    // Start this server's own worker; it probes for rc-astro first.
    {
        let state = Arc::clone(&state);
        tokio::task::spawn_blocking(move || state.workers.refresh_local());
    }

    // Per-DB routes — nested under /api/db/{db_id}/.
    let api_routes = api_router(Arc::clone(&state));

    // Create main app with either embedded or filesystem static serving
    let app = if let Some(static_dir_path) = &config.static_dir {
        // Use filesystem static serving (for development) with proper MIME types
        let static_path = PathBuf::from(static_dir_path);
        let page = Arc::new(PageBuild::in_dir(static_path.clone()));
        let static_service = StaticFileService::new(static_path);

        tracing::info!("Serving static files from filesystem: {}", static_dir_path);

        Router::new()
            .nest("/api", api_routes)
            .fallback_service(static_service)
            .layer(axum::middleware::from_fn_with_state(
                page,
                page_build::stamp,
            ))
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
            .layer(axum::middleware::from_fn_with_state(
                Arc::new(PageBuild::Embedded),
                page_build::stamp,
            ))
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http())
                    .layer(CorsLayer::permissive()),
            )
    };

    // Create listener
    let listener = match listener {
        Some(listener) => listener,
        None => listener::SharedTcpListener::new(
            tokio::net::TcpListener::bind(format!("{}:{}", config.host, config.port)).await?,
        )?,
    };

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
    if let Some(ready) = ready {
        let _ = ready.send(());
    }

    // Run server with optional graceful shutdown
    let stopping_state = Arc::clone(&journal_state);
    match shutdown_rx {
        Some(shutdown_rx) => {
            tracing::info!("🚀 Server started with graceful shutdown support");
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    shutdown_rx.await.ok();
                    tracing::info!("🛑 Graceful shutdown signal received");
                    begin_stop(&stopping_state);
                })
                .await?;
        }
        None => {
            // Ctrl+C and a service manager's SIGTERM end the server cleanly,
            // so the job journal gets its last word.
            axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    stop_signal().await;
                    tracing::info!("🛑 Stop signal received");
                    begin_stop(&stopping_state);
                })
                .await?;
        }
    }

    finish_stop(&journal_state).await;
    tracing::info!("🛑 Server shutdown completed");
    Ok(())
}

/// Tell background work the server is stopping: loops start nothing new,
/// and stack builds stop at their next checkpoint.
fn begin_stop(state: &AppState) {
    state.shutdown.begin();
    state.stack_previews.stop_for_shutdown();
}

/// Wait for the work in hand to finish, then leave the job journal for the
/// next start.
async fn finish_stop(state: &AppState) {
    // As a crash would leave it, in case something kills the process before
    // the work below finishes: a build still running counts as cut off.
    crate::server::stack_preview::journal::write_now(state);
    state.shutdown.drained().await;
    // Builds that stopped at a checkpoint come back without counting as cut
    // off.
    crate::server::stack_preview::journal::write_now(state);
}

/// Ctrl+C, or SIGTERM where there is one.
async fn stop_signal() {
    let interrupt = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = interrupt => {}
        _ = terminate => {}
    }
}

pub async fn run_server_with_shutdown(
    config: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    // Don't initialize tracing here - it should already be initialized by the first server or Tauri app
    run_server_internal(config, Some(shutdown_rx), None, None).await
}

/// Desktop startup reserves its socket before opening the UI, and reports
/// readiness only once the API router and catalog state are ready.
#[cfg(feature = "tauri")]
pub(crate) async fn run_server_on_listener(
    config: ServerConfig,
    shutdown_rx: oneshot::Receiver<()>,
    listener: listener::SharedTcpListener,
    ready: oneshot::Sender<()>,
) -> anyhow::Result<()> {
    run_server_internal(config, Some(shutdown_rx), Some(listener), Some(ready)).await
}

/// Say what previews will be written as, and how much of the cache is in the
/// other format.
///
/// Changing the format leaves the old artifacts in place on purpose — the two
/// use different file names, so a change misses and regenerates rather than
/// serving one as the other, and changing back finds the originals valid. The
/// cost is that both sets sit on disk until somebody removes one, and an
/// operator who switched to JPEG for the disk space should be told that the
/// PNGs are still there rather than discovering it later.
fn report_preview_settings(state: &AppState, config: &ServerConfig) {
    let encoding = config.preview_encoding;
    tracing::info!(
        "🖼️ Previews: {} ({}), colour {} by default",
        encoding.extension(),
        match encoding.format {
            crate::preview_format::PreviewFormat::Png => "exact".to_string(),
            crate::preview_format::PreviewFormat::Jpeg =>
                format!("lossy, quality {}", encoding.jpeg_quality),
        },
        if config.preview_color_default {
            "on"
        } else {
            "off"
        }
    );

    let stale = state
        .all_databases()
        .iter()
        .map(|ctx| stale_format_bytes(&ctx.cache_dir_path, encoding.format))
        .sum::<u64>();
    if stale > 0 {
        tracing::warn!(
            "🖼️ {:.1} MiB of cached previews are in the other format and will \
             not be served or reused. Remove them if the disk matters.",
            stale as f64 / (1024.0 * 1024.0)
        );
    }
}

/// Bytes of cached preview artifacts that are *not* in the configured format.
fn stale_format_bytes(
    cache_dir: &std::path::Path,
    keeping: crate::preview_format::PreviewFormat,
) -> u64 {
    crate::server::storage::ENCODED_PREVIEW_CATEGORIES
        .iter()
        .flat_map(|category| std::fs::read_dir(cache_dir.join(category)))
        .flatten()
        .flatten()
        .filter(|entry| crate::preview_format::PreviewFormat::of_path(&entry.path()) != keeping)
        .filter_map(|entry| entry.metadata().ok().map(|meta| meta.len()))
        .sum()
}

/// Default JSON API responses to `Cache-Control: no-store` when the handler
/// set no policy of its own.
///
/// Analysis endpoints return sequence-relative scores that change whenever a
/// quality scan lands new evidence. Without a cache policy, an embedding
/// webview (the desktop app) may serve its own cached copy of one view's
/// request while another view fetches fresh — the same image then shows two
/// different scores depending on the page. Binary responses (previews,
/// thumbnails) keep their own explicit policies and are not touched.
async fn json_no_store(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let mut response = next.run(request).await;
    apply_json_no_store(&mut response);
    response
}

/// The policy itself, separated so it can be tested without a router.
fn apply_json_no_store(response: &mut axum::response::Response) {
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if is_json && !response.headers().contains_key(CACHE_CONTROL) {
        response.headers_mut().insert(
            CACHE_CONTROL,
            axum::http::HeaderValue::from_static("no-store"),
        );
    }
}

#[cfg(test)]
mod cache_policy_tests {
    use super::apply_json_no_store;
    use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};

    fn response(content_type: &str) -> axum::response::Response {
        axum::response::Response::builder()
            .header(CONTENT_TYPE, content_type)
            .body(axum::body::Body::empty())
            .unwrap()
    }

    #[test]
    fn json_without_a_policy_becomes_no_store() {
        let mut r = response("application/json");
        apply_json_no_store(&mut r);
        assert_eq!(r.headers()[CACHE_CONTROL], "no-store");
    }

    #[test]
    fn explicit_policies_and_binary_responses_are_untouched() {
        let mut r = axum::response::Response::builder()
            .header(CONTENT_TYPE, "application/json")
            .header(CACHE_CONTROL, "max-age=5")
            .body(axum::body::Body::empty())
            .unwrap();
        apply_json_no_store(&mut r);
        assert_eq!(r.headers()[CACHE_CONTROL], "max-age=5");

        let mut image = response("image/png");
        apply_json_no_store(&mut image);
        assert!(!image.headers().contains_key(CACHE_CONTROL));
    }
}

#[cfg(test)]
mod startup_tests {
    use super::{anonymous_access_is_trusted, check_management_needs_a_login};

    /// The startup pair, as `run_server_internal` computes it: work out who a
    /// session-less caller is, then decide whether the server may start.
    fn start(host: &str, manage: bool, anonymous: bool, has_users: bool) -> anyhow::Result<bool> {
        let trusted = anonymous_access_is_trusted(host, anonymous);
        check_management_needs_a_login(host, manage, trusted, has_users)?;
        Ok(trusted)
    }

    #[test]
    fn a_network_server_cannot_offer_database_management_without_a_login() {
        let refusal = start("0.0.0.0", true, false, false).unwrap_err();
        assert!(refusal.to_string().contains("psf-guard users add"));

        // A login exists, so the editor gate applies as usual.
        assert!(start("0.0.0.0", true, false, true).is_ok());
        // No management asked for, so nothing to hand out.
        assert!(start("0.0.0.0", false, false, false).is_ok());
    }

    #[test]
    fn only_loopback_or_the_explicit_flag_trusts_a_caller_with_no_account() {
        // The desktop app and local development.
        assert!(start("127.0.0.1", true, false, false).unwrap());
        assert!(start("localhost", true, false, false).unwrap());
        // A network server stays closed until an operator says otherwise.
        assert!(!start("0.0.0.0", false, false, false).unwrap());
        // The operator said otherwise, so management may start too.
        assert!(start("0.0.0.0", true, true, false).unwrap());
    }
}
