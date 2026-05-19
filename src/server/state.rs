use crate::cli::PregenerationConfig;
use crate::directory_tree::DirectoryTree;
use crate::server::database_context::DatabaseContext;
use crate::server::slug::compute_default_slug;
use anyhow::Result;
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;

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
/// Multi-DB lives in `databases` (keyed by slug). All per-DB operations route
/// through `DatabaseContext`. The legacy public fields and methods below
/// (`database_path`, `image_dirs`, `db()`, `file_check_cache`, etc.) are
/// compatibility shims that delegate to the **single** entry in `databases`.
/// They're kept until B2 nests handlers under `/api/db/{slug}/...` and takes
/// a `DbContext` extractor directly.
pub struct AppState {
    /// Canonical multi-DB state, keyed by slug.
    pub databases: RwLock<HashMap<String, Arc<DatabaseContext>>>,
    /// Pre-generation configuration (process-global).
    pub pregeneration_config: PregenerationConfig,
    /// Root cache directory; per-DB caches live under this. In B1 this is
    /// also where each context writes (no namespacing yet — see B5).
    pub cache_dir_root: String,

    // ── Compatibility shims (B1) ────────────────────────────────────────────
    // These mirror fields of the single DatabaseContext so existing handlers
    // continue to compile without modification. Removed in B2.
    pub database_path: String,
    pub image_dirs: Vec<String>,
    pub cache_dir: String,
    pub file_check_cache: Arc<RwLock<FileCheckCache>>,
    pub directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>,
    pub refresh_mutex: Arc<TokioMutex<()>>,
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
    pub fn new(
        db_path: String,
        image_dirs: Vec<String>,
        cache_dir: String,
        pregeneration_config: PregenerationConfig,
    ) -> Result<Self> {
        // Build the single DatabaseContext using a default slug from the path.
        let slug = compute_default_slug(&db_path);
        let display_name = PathBuf::from(&db_path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Database".to_string());

        let ctx = Arc::new(DatabaseContext::new(
            slug.clone(),
            display_name,
            db_path.clone(),
            image_dirs.clone(),
            cache_dir.clone(),
        )?);

        // Compat shim fields point at the same Arcs the context owns, so
        // mutations through either path are seen by both.
        let file_check_cache = Arc::clone(&ctx.file_check_cache);
        let directory_tree_cache = Arc::clone(&ctx.directory_tree_cache);
        let refresh_mutex = Arc::clone(&ctx.refresh_mutex);

        let mut databases = HashMap::new();
        databases.insert(slug, ctx);

        Ok(Self {
            databases: RwLock::new(databases),
            pregeneration_config,
            cache_dir_root: cache_dir.clone(),
            database_path: db_path,
            image_dirs,
            cache_dir,
            file_check_cache,
            directory_tree_cache,
            refresh_mutex,
        })
    }

    // ── Multi-DB primitives (used by B2+) ────────────────────────────────────

    /// Look up a database by slug.
    pub fn get_database(&self, slug: &str) -> Option<Arc<DatabaseContext>> {
        self.databases.read().unwrap().get(slug).cloned()
    }

    /// Get every configured database context.
    pub fn all_databases(&self) -> Vec<Arc<DatabaseContext>> {
        self.databases.read().unwrap().values().cloned().collect()
    }

    /// Get the (singular) database context. **Panics** if there isn't exactly
    /// one — only valid during B1, where all handlers still assume single-DB.
    /// Removed when B2 introduces the path-nested API and `DbContext` extractor.
    pub fn single_context(&self) -> Arc<DatabaseContext> {
        let dbs = self.databases.read().unwrap();
        if dbs.len() != 1 {
            panic!(
                "single_context() called with {} databases configured — B2 \
                 must convert the calling handler to use DbContext",
                dbs.len()
            );
        }
        dbs.values().next().cloned().unwrap()
    }

    // ── Compatibility shims (B1) ────────────────────────────────────────────
    // These delegate to the single DatabaseContext so existing handlers
    // compile unchanged.

    pub fn db(&self) -> Arc<Mutex<Connection>> {
        self.single_context().db()
    }

    pub fn get_cache_path(&self, category: &str, filename: &str) -> PathBuf {
        self.single_context().get_cache_path(category, filename)
    }

    pub fn get_image_path(&self, relative_path: &str) -> PathBuf {
        self.single_context().get_image_path(relative_path)
    }

    pub fn get_directory_tree(&self) -> Result<Arc<DirectoryTree>> {
        self.single_context().get_directory_tree()
    }

    pub fn refresh_directory_tree_if_needed(&self) -> Result<Arc<DirectoryTree>> {
        self.single_context().refresh_directory_tree_if_needed()
    }

    pub fn clear_directory_tree_cache(&self) {
        self.single_context().clear_directory_tree_cache()
    }

    pub fn get_directory_tree_stats(&self) -> Option<crate::directory_tree::DirectoryTreeStats> {
        self.single_context().get_directory_tree_stats()
    }

    pub fn ensure_cache_available(&self) -> RefreshStatus {
        self.single_context().ensure_cache_available()
    }

    pub fn force_directory_tree_refresh(&self) -> RefreshStatus {
        self.single_context().force_directory_tree_refresh()
    }

    pub fn get_cache_refresh_progress(&self) -> Option<RefreshProgress> {
        self.single_context().get_cache_refresh_progress()
    }
}

impl AppState {
    /// Create an AppState for integration testing with a pre-opened database connection.
    /// Skips file system validation (no image dirs or cache dir needed).
    #[doc(hidden)]
    pub fn new_for_test(conn: Connection) -> Self {
        let ctx = Arc::new(DatabaseContext::new_for_test(conn));
        let file_check_cache = Arc::clone(&ctx.file_check_cache);
        let directory_tree_cache = Arc::clone(&ctx.directory_tree_cache);
        let refresh_mutex = Arc::clone(&ctx.refresh_mutex);
        let slug = ctx.id.clone();

        let mut databases = HashMap::new();
        databases.insert(slug, ctx);

        Self {
            databases: RwLock::new(databases),
            pregeneration_config: crate::cli::PregenerationConfig::default(),
            cache_dir_root: "/tmp/psf-guard-test".to_string(),
            database_path: ":memory:".to_string(),
            image_dirs: vec![],
            cache_dir: "/tmp/psf-guard-test".to_string(),
            file_check_cache,
            directory_tree_cache,
            refresh_mutex,
        }
    }
}
