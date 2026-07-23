use crate::cli::PregenerationConfig;
use crate::db_registry::DbEntry;
use crate::server::database_context::DatabaseContext;
use crate::server::slug::compute_default_slug;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum RefreshStatus {
    /// No refresh needed, cache is fresh
    NotNeeded,
    /// Refresh in progress, serve stale data if available
    InProgressServeStale,
    /// Refresh in progress, no data available - frontend should wait/show loading
    InProgressWait,
    /// Cache is empty and no refresh in progress - need to start refresh
    NeedsRefresh,
}

/// Process-global server state.
///
/// All per-database work routes through entries in `databases`, keyed by slug.
/// Handlers reach a specific database via the `DbContext` extractor which
/// looks up by the `{db_id}` path segment.
pub struct AppState {
    /// Canonical multi-DB state, keyed by slug.
    pub databases: RwLock<HashMap<String, Arc<DatabaseContext>>>,
    /// Pre-generation configuration (process-global).
    pub pregeneration_config: PregenerationConfig,
    /// Root cache directory; per-DB caches live under this. B5 will namespace
    /// per-slug subdirectories beneath this root.
    pub cache_dir_root: String,
    /// Path to the on-disk database registry that mirrors `databases`. When
    /// set, the CRUD endpoints (`POST/PUT/DELETE /api/databases/...`) persist
    /// changes here. `None` disables the CRUD endpoints (e.g. in tests).
    pub registry_path: RwLock<Option<PathBuf>>,
    /// Whether HTTP clients are allowed to call the database CRUD endpoints.
    /// Required *in addition to* `registry_path` being set, so an
    /// untrustworthy client cannot mutate the user's configuration even if
    /// the server has a registry to persist to.
    pub allow_database_management: RwLock<bool>,
    /// Optional plain-text notice displayed below the application header.
    pub site_banner: RwLock<Option<crate::config::SiteBannerConfig>>,
    /// Tuning policy for the parallel scans and background pre-generation (see
    /// `concurrency::WorkerPolicy`). Process-global; sourced from the TOML
    /// `[server]` ratios, otherwise the compiled-in defaults.
    pub worker_policy: RwLock<crate::concurrency::WorkerPolicy>,
    /// Count of interactive (user-triggered) CPU-heavy jobs currently running,
    /// process-wide. Background work reads this to yield: while it is nonzero,
    /// pre-generation pauses so it doesn't compete for cores or memory with a
    /// scan the user is waiting on. An `Arc` so a [`InteractiveJobGuard`] can
    /// decrement it on drop from a `spawn_blocking` task.
    active_interactive_jobs: Arc<AtomicUsize>,
    /// Bounded, interactive-priority queue for on-demand preview / annotated
    /// PNG generation (see `preview_queue`). Process-global so total concurrent
    /// generation is bounded regardless of how many databases are loaded.
    pub preview_queue: crate::server::preview_queue::PreviewQueue,
    /// Process-global, single-flight project stacking preview jobs. Full-frame
    /// stacking is memory intensive, so groups and databases share one permit.
    pub stack_previews: crate::server::stack_preview::StackPreviewManager,
    /// Process-global Seiza catalogs and capability diagnostics. Catalogs are
    /// shared across databases and opened lazily on first use.
    pub astrometry: Arc<crate::astrometry::AstrometryContext>,
    /// Process-global orbital-element cache and satellite predictor. Network
    /// refresh is explicit; sequence grading consumes cached predictions only.
    pub satellites: Arc<crate::satellites::SatelliteContext>,
}

/// RAII marker that an interactive CPU-heavy job is running. Increments the
/// process-wide gauge on creation and decrements on drop, so background work
/// yields for exactly the job's lifetime even if it panics.
pub struct InteractiveJobGuard(Arc<AtomicUsize>);

impl Drop for InteractiveJobGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Clone)]
pub struct FileCheckCache {
    pub projects_with_files: HashMap<i32, bool>,
    pub targets_with_files: HashMap<i32, bool>,
    pub last_updated: Instant,
    pub cache_duration: Duration,
    pub refresh_in_progress: bool,
    pub has_initial_data: bool,
    pub refresh_progress: RefreshProgress,
}

#[derive(Clone, Debug)]
pub struct RefreshProgress {
    pub stage: RefreshStage,
    pub start_time: Option<Instant>,
    pub directories_total: usize,
    pub directories_processed: usize,
    pub current_directory_name: Option<String>,
    pub files_scanned: usize, // Files discovered during directory scanning
    pub projects_total: usize,
    pub projects_processed: usize,
    pub current_project_name: Option<String>,
    pub targets_total: usize,
    pub targets_processed: usize,
    pub files_found: usize,
    pub files_missing: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RefreshStage {
    Idle,
    InitializingDirectoryTree,
    LoadingProjects,
    ProcessingProjects,
    ProcessingTargets,
    UpdatingCache,
    Completed,
}

impl Default for RefreshProgress {
    fn default() -> Self {
        Self {
            stage: RefreshStage::Idle,
            start_time: None,
            directories_total: 0,
            directories_processed: 0,
            current_directory_name: None,
            files_scanned: 0,
            projects_total: 0,
            projects_processed: 0,
            current_project_name: None,
            targets_total: 0,
            targets_processed: 0,
            files_found: 0,
            files_missing: 0,
        }
    }
}

impl RefreshProgress {
    pub fn start_refresh(&mut self) {
        *self = Self {
            stage: RefreshStage::InitializingDirectoryTree,
            start_time: Some(Instant::now()),
            ..Default::default()
        };
    }

    pub fn set_stage(&mut self, stage: RefreshStage) {
        self.stage = stage;
    }

    pub fn set_directories_info(&mut self, total: usize) {
        self.directories_total = total;
        self.directories_processed = 0;
    }

    pub fn process_directory(&mut self, directory_name: &str) {
        self.current_directory_name = Some(directory_name.to_string());
    }

    pub fn complete_directory(&mut self) {
        self.directories_processed += 1;
    }

    pub fn set_projects_info(&mut self, total: usize) {
        self.projects_total = total;
        self.projects_processed = 0;
        self.stage = RefreshStage::ProcessingProjects;
    }

    pub fn process_project(&mut self, project_name: &str) {
        self.current_project_name = Some(project_name.to_string());
    }

    pub fn complete_project(&mut self, has_files: bool, files_found: usize, files_missing: usize) {
        self.projects_processed += 1;
        if has_files {
            self.files_found += files_found;
            self.files_missing += files_missing;
        }
    }

    pub fn set_targets_info(&mut self, additional_targets: usize) {
        self.targets_total += additional_targets;
        if self.stage != RefreshStage::ProcessingTargets {
            self.stage = RefreshStage::ProcessingTargets;
        }
    }

    pub fn complete_target(&mut self, has_files: bool, files_found: usize, files_missing: usize) {
        self.targets_processed += 1;
        if has_files {
            self.files_found += files_found;
            self.files_missing += files_missing;
        }
    }

    pub fn complete_refresh(&mut self) {
        self.stage = RefreshStage::Completed;
        self.current_project_name = None;
        self.current_directory_name = None;
    }

    pub fn update_files_scanned(&mut self, files_scanned: usize) {
        self.files_scanned = files_scanned;
    }

    pub fn get_progress_percentage(&self) -> f32 {
        match self.stage {
            RefreshStage::Idle => 0.0,
            RefreshStage::InitializingDirectoryTree => {
                if self.directories_total > 0 {
                    2.0 + (self.directories_processed as f32 / self.directories_total as f32) * 8.0
                } else {
                    5.0
                }
            }
            RefreshStage::LoadingProjects => 10.0,
            RefreshStage::ProcessingProjects => {
                if self.projects_total > 0 {
                    15.0 + (self.projects_processed as f32 / self.projects_total as f32) * 50.0
                } else {
                    15.0
                }
            }
            RefreshStage::ProcessingTargets => {
                if self.targets_total > 0 {
                    65.0 + (self.targets_processed as f32 / self.targets_total as f32) * 25.0
                } else {
                    65.0
                }
            }
            RefreshStage::UpdatingCache => 95.0,
            RefreshStage::Completed => 100.0,
        }
    }

    pub fn get_elapsed_time(&self) -> Option<Duration> {
        self.start_time.map(|start| start.elapsed())
    }
}

impl Default for FileCheckCache {
    fn default() -> Self {
        Self::new()
    }
}

impl FileCheckCache {
    pub fn new() -> Self {
        Self {
            projects_with_files: HashMap::new(),
            targets_with_files: HashMap::new(),
            last_updated: Instant::now(),
            cache_duration: Duration::from_secs(60), // 1 minute cache
            refresh_in_progress: false,
            has_initial_data: false,
            refresh_progress: RefreshProgress::default(),
        }
    }

    pub fn is_expired(&self) -> bool {
        self.last_updated.elapsed() > self.cache_duration
    }

    pub fn clear(&mut self) {
        self.projects_with_files.clear();
        self.targets_with_files.clear();
        self.last_updated = Instant::now();
        self.refresh_in_progress = false;
        self.has_initial_data = false;
        self.refresh_progress = RefreshProgress::default();
    }

    pub fn mark_refresh_started(&mut self) {
        self.refresh_in_progress = true;
        self.refresh_progress.start_refresh();
    }

    pub fn mark_refresh_completed(&mut self) {
        self.refresh_in_progress = false;
        self.has_initial_data = true;
        self.last_updated = Instant::now();
        self.refresh_progress.complete_refresh();
    }

    pub fn should_serve_stale(&self) -> bool {
        // Serve stale data if we have initial data and refresh is in progress
        self.has_initial_data && self.refresh_in_progress
    }

    pub fn get_refresh_status(&self) -> RefreshStatus {
        if self.refresh_in_progress {
            if self.has_initial_data {
                RefreshStatus::InProgressServeStale
            } else {
                RefreshStatus::InProgressWait
            }
        } else if self.is_expired()
            || (!self.has_initial_data
                && self.projects_with_files.is_empty()
                && self.targets_with_files.is_empty())
        {
            RefreshStatus::NeedsRefresh
        } else {
            RefreshStatus::NotNeeded
        }
    }
}

impl AppState {
    /// Build state for N configured databases. Each entry opens its own
    /// SQLite connection; failures bubble up immediately.
    pub fn from_databases(
        databases: Vec<DbEntry>,
        cache_dir: String,
        pregeneration_config: PregenerationConfig,
    ) -> Result<Self> {
        Self::from_databases_with_astrometry(databases, cache_dir, pregeneration_config, None)
    }

    /// Build state with optional process-global Seiza catalog configuration.
    pub fn from_databases_with_astrometry(
        databases: Vec<DbEntry>,
        cache_dir: String,
        pregeneration_config: PregenerationConfig,
        astrometry_config: Option<crate::astrometry::AstrometryConfig>,
    ) -> Result<Self> {
        let astrometry_config = astrometry_config.unwrap_or_default();
        let satellites = crate::satellites::SatelliteContext::new(
            PathBuf::from(&cache_dir).join("satellites"),
            astrometry_config.satellite_elements_path(),
        )
        .map_err(anyhow::Error::msg)?;
        let mut map = HashMap::with_capacity(databases.len());
        for entry in databases {
            let ctx = Arc::new(DatabaseContext::new(
                entry.id.clone(),
                entry.name,
                entry.db_path,
                entry.image_dirs,
                cache_dir.clone(),
            )?);
            map.insert(entry.id, ctx);
        }

        Ok(Self {
            databases: RwLock::new(map),
            pregeneration_config,
            cache_dir_root: cache_dir,
            registry_path: RwLock::new(None),
            allow_database_management: RwLock::new(false),
            site_banner: RwLock::new(None),
            worker_policy: RwLock::new(crate::concurrency::WorkerPolicy::default()),
            active_interactive_jobs: Arc::new(AtomicUsize::new(0)),
            preview_queue: crate::server::preview_queue::PreviewQueue::default(),
            stack_previews: crate::server::stack_preview::StackPreviewManager::default(),
            astrometry: Arc::new(crate::astrometry::AstrometryContext::new(astrometry_config)),
            satellites: Arc::new(satellites),
        })
    }

    /// Attach the path of the on-disk registry that mirrors this state.
    /// Required (but not sufficient) for the CRUD endpoints to function — see
    /// also `set_allow_database_management`.
    pub fn set_registry_path(&self, path: Option<PathBuf>) {
        *self.registry_path.write().unwrap() = path;
    }

    /// Toggle the database CRUD endpoints. When false, mutating routes on
    /// `/api/databases` return 403.
    pub fn set_allow_database_management(&self, allow: bool) {
        *self.allow_database_management.write().unwrap() = allow;
    }

    pub fn database_management_allowed(&self) -> bool {
        *self.allow_database_management.read().unwrap()
    }

    pub fn set_site_banner(&self, banner: Option<crate::config::SiteBannerConfig>) {
        *self.site_banner.write().unwrap() = banner;
    }

    pub fn site_banner(&self) -> Option<crate::config::SiteBannerConfig> {
        self.site_banner.read().unwrap().clone()
    }

    /// Set the worker tuning policy (from the TOML `[server]` config).
    pub fn set_worker_policy(&self, policy: crate::concurrency::WorkerPolicy) {
        *self.worker_policy.write().unwrap() = policy;
    }

    /// The worker tuning policy in effect.
    pub fn worker_policy(&self) -> crate::concurrency::WorkerPolicy {
        *self.worker_policy.read().unwrap()
    }

    /// Mark the start of an interactive CPU-heavy job (e.g. an occlusion
    /// scan). Hold the returned guard for the job's lifetime; background work
    /// yields while any guard is alive.
    pub fn begin_interactive_job(&self) -> InteractiveJobGuard {
        self.active_interactive_jobs.fetch_add(1, Ordering::SeqCst);
        InteractiveJobGuard(Arc::clone(&self.active_interactive_jobs))
    }

    /// Whether any interactive CPU-heavy job is currently running. Background
    /// work checks this and pauses so it doesn't compete for cores or memory.
    pub fn interactive_job_active(&self) -> bool {
        self.active_interactive_jobs.load(Ordering::SeqCst) > 0
    }

    /// Convenience constructor for a single database. Computes a default slug
    /// from the path. Used by callers that don't go through `DbRegistry`.
    pub fn new(
        db_path: String,
        image_dirs: Vec<String>,
        cache_dir: String,
        pregeneration_config: PregenerationConfig,
    ) -> Result<Self> {
        let slug = compute_default_slug(&db_path);
        let name = PathBuf::from(&db_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "Database".to_string());
        Self::from_databases(
            vec![DbEntry {
                id: slug,
                name,
                db_path,
                image_dirs,
                reject_archive: None,
            }],
            cache_dir,
            pregeneration_config,
        )
    }

    /// Look up a database by slug.
    pub fn get_database(&self, slug: &str) -> Option<Arc<DatabaseContext>> {
        self.databases.read().unwrap().get(slug).cloned()
    }

    /// Get every configured database context.
    pub fn all_databases(&self) -> Vec<Arc<DatabaseContext>> {
        self.databases.read().unwrap().values().cloned().collect()
    }
}

impl AppState {
    /// Create an AppState for integration testing with a pre-opened database connection.
    /// Skips file system validation (no image dirs or cache dir needed).
    #[doc(hidden)]
    pub fn new_for_test(conn: Connection) -> Self {
        let ctx = Arc::new(DatabaseContext::new_for_test(conn));
        let slug = ctx.id.clone();

        let mut databases = HashMap::new();
        databases.insert(slug, ctx);

        Self {
            databases: RwLock::new(databases),
            pregeneration_config: crate::cli::PregenerationConfig::default(),
            cache_dir_root: "/tmp/psf-guard-test".to_string(),
            registry_path: RwLock::new(None),
            allow_database_management: RwLock::new(false),
            site_banner: RwLock::new(None),
            worker_policy: RwLock::new(crate::concurrency::WorkerPolicy::default()),
            active_interactive_jobs: Arc::new(AtomicUsize::new(0)),
            preview_queue: crate::server::preview_queue::PreviewQueue::default(),
            stack_previews: crate::server::stack_preview::StackPreviewManager::default(),
            astrometry: Arc::new(crate::astrometry::AstrometryContext::default()),
            satellites: Arc::new(
                crate::satellites::SatelliteContext::new(
                    PathBuf::from("/tmp/psf-guard-test/satellites"),
                    None,
                )
                .expect("test satellite context"),
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state() -> AppState {
        AppState::new_for_test(Connection::open_in_memory().unwrap())
    }

    #[test]
    fn interactive_job_gauge_tracks_guard_lifetime() {
        let state = test_state();
        // No interactive work initially — background may run.
        assert!(!state.interactive_job_active());

        {
            let _guard = state.begin_interactive_job();
            // A live guard means background work should yield.
            assert!(state.interactive_job_active());
        }
        // Guard dropped -> gauge clears -> background may resume.
        assert!(!state.interactive_job_active());
    }

    #[test]
    fn site_banner_round_trips_through_state() {
        let state = test_state();
        assert!(state.site_banner().is_none());

        let banner = crate::config::SiteBannerConfig {
            title: "Demo site".into(),
            message: "Sample data".into(),
            link_text: None,
            link_url: None,
        };
        state.set_site_banner(Some(banner.clone()));
        assert_eq!(state.site_banner(), Some(banner));
    }

    #[test]
    fn interactive_job_gauge_nests() {
        // Concurrent scans (e.g. across databases) must both have to finish
        // before background work resumes; the gauge is a count, not a flag.
        let state = test_state();
        let g1 = state.begin_interactive_job();
        let g2 = state.begin_interactive_job();
        assert!(state.interactive_job_active());
        drop(g1);
        assert!(
            state.interactive_job_active(),
            "still active while one job remains"
        );
        drop(g2);
        assert!(!state.interactive_job_active());
    }

    #[test]
    fn interactive_guard_decrements_even_on_panic() {
        // The whole point of the RAII guard: a panicking scan must not leave
        // the gauge stuck > 0 and permanently starve background pre-generation.
        let state = std::sync::Arc::new(test_state());
        let state2 = std::sync::Arc::clone(&state);
        let handle = std::thread::spawn(move || {
            let _guard = state2.begin_interactive_job();
            assert!(state2.interactive_job_active());
            panic!("scan blew up");
        });
        assert!(handle.join().is_err(), "thread should have panicked");
        assert!(
            !state.interactive_job_active(),
            "guard must decrement while unwinding"
        );
    }
}
