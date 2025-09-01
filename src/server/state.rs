use crate::cli::PregenerationConfig;
use crate::directory_tree::DirectoryTree;
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
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

pub struct AppState {
    pub database_path: String,
    pub image_dirs: Vec<String>,
    pub cache_dir: String,
    image_dir_paths: Vec<PathBuf>,
    cache_dir_path: PathBuf,
    // We'll use a connection pool or create connections as needed
    db_connection: Arc<Mutex<Connection>>,
    // Cache for file existence checks
    pub file_check_cache: Arc<RwLock<FileCheckCache>>,
    // Directory tree cache for fast file lookups
    pub directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>,
    // Background refresh coordination
    pub refresh_mutex: Arc<TokioMutex<()>>,
    // Pre-generation configuration
    pub pregeneration_config: PregenerationConfig,
}

#[derive(Clone)]
pub struct FileCheckCache {
    pub projects_with_files: HashMap<i32, bool>,
    pub targets_with_files: HashMap<i32, bool>,
    pub last_updated: Instant,
    pub cache_duration: Duration,
    pub refresh_in_progress: bool,
    pub has_initial_data: bool,
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
    }

    pub fn mark_refresh_started(&mut self) {
        self.refresh_in_progress = true;
    }

    pub fn mark_refresh_completed(&mut self) {
        self.refresh_in_progress = false;
        self.has_initial_data = true;
        self.last_updated = Instant::now();
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
        use std::path::Path;

        // Check if database exists
        if !Path::new(&db_path).exists() {
            return Err(anyhow::anyhow!("Database file not found: {}", db_path));
        }

        // Check if image directories exist
        let mut image_dir_paths = Vec::new();
        for dir in &image_dirs {
            let path = Path::new(dir);
            if !path.exists() {
                return Err(anyhow::anyhow!("Image directory not found: {}", dir));
            }
            image_dir_paths.push(PathBuf::from(dir));
        }

        if image_dirs.is_empty() {
            return Err(anyhow::anyhow!(
                "At least one image directory must be specified"
            ));
        }

        // Open database connection
        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        Ok(Self {
            database_path: db_path.clone(),
            image_dirs: image_dirs.clone(),
            cache_dir: cache_dir.clone(),
            image_dir_paths,
            cache_dir_path: PathBuf::from(cache_dir),
            db_connection: Arc::new(Mutex::new(conn)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            pregeneration_config,
        })
    }

    pub fn db(&self) -> Arc<Mutex<Connection>> {
        self.db_connection.clone()
    }

    pub fn get_cache_path(&self, category: &str, filename: &str) -> PathBuf {
        self.cache_dir_path.join(category).join(filename)
    }

    pub fn get_image_path(&self, relative_path: &str) -> PathBuf {
        // Return path for the first directory for compatibility
        // File lookup should use the directory tree cache for multi-directory support
        self.image_dir_paths[0].join(relative_path)
    }

    /// Get or build the directory tree cache
    pub fn get_directory_tree(&self) -> Result<Arc<DirectoryTree>> {
        // First, try to read the existing cache
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                // Check if cache is still valid (not too old)
                if !tree.is_older_than(Duration::from_secs(300)) {
                    // 5 minute cache
                    return Ok(Arc::new(tree.clone()));
                }
            }
        }

        // Need to rebuild cache
        self.rebuild_directory_tree()
    }

    /// Force rebuild of the directory tree cache
    pub fn rebuild_directory_tree(&self) -> Result<Arc<DirectoryTree>> {
        tracing::info!(
            "🌳 Building directory tree cache for {} directories",
            self.image_dirs.len()
        );
        let roots: Vec<&std::path::Path> =
            self.image_dir_paths.iter().map(|p| p.as_path()).collect();
        let tree = DirectoryTree::build_multiple(&roots)?;
        let stats = tree.stats();

        tracing::info!(
            "✅ Directory tree built: {} files, {} directories across {} roots (age: {})",
            stats.total_files,
            stats.total_directories,
            stats.roots.len(),
            stats.format_age()
        );

        // Store in cache
        {
            let mut cache = self.directory_tree_cache.write().unwrap();
            *cache = Some(tree.clone());
        }

        Ok(Arc::new(tree))
    }

    /// Refresh directory tree cache only if needed (not recently built)
    pub fn refresh_directory_tree_if_needed(&self) -> Result<Arc<DirectoryTree>> {
        // Check if we have a valid cache first
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                // Check if cache is still valid (not too old)
                if !tree.is_older_than(Duration::from_secs(300)) {
                    // Cache is fresh, no need to rebuild
                    tracing::debug!("🌳 Directory tree cache is fresh, skipping rebuild");
                    return Ok(Arc::new(tree.clone()));
                } else {
                    tracing::debug!("🌳 Directory tree cache is stale (>5min), rebuilding");
                }
            } else {
                tracing::debug!("🌳 Directory tree cache is empty, building");
            }
        }

        // Cache is stale or empty, rebuild it
        self.rebuild_directory_tree()
    }

    /// Clear the directory tree cache (force rebuild on next access)
    pub fn clear_directory_tree_cache(&self) {
        let mut cache = self.directory_tree_cache.write().unwrap();
        *cache = None;
        tracing::info!("🗑️  Directory tree cache cleared");
    }

    /// Get directory tree cache statistics
    pub fn get_directory_tree_stats(&self) -> Option<crate::directory_tree::DirectoryTreeStats> {
        let cache = self.directory_tree_cache.read().unwrap();
        cache.as_ref().map(|tree| tree.stats())
    }

    /// Check if a cache refresh is needed and start one if necessary
    /// Returns status without blocking
    pub fn ensure_cache_available(&self) -> RefreshStatus {
        // Quick check without blocking
        let status = {
            let cache = self.file_check_cache.read().unwrap();
            cache.get_refresh_status()
        };

        match status {
            RefreshStatus::NeedsRefresh => {
                // Try to start refresh without blocking
                self.spawn_background_refresh()
            }
            _ => status,
        }
    }

    /// Unified cache refresh operation - handles all cache refresh needs
    /// This should only be called from the singleton background task
    async fn refresh_cache_unified_internal(
        &self,
    ) -> Result<(usize, usize, usize, u128), anyhow::Error> {
        let start_time = std::time::Instant::now();
        tracing::info!("🔄 Starting unified cache refresh");

        // First, refresh the directory tree cache to ensure file lookups are up-to-date
        if let Err(e) = self.refresh_directory_tree_if_needed() {
            tracing::warn!(
                "⚠️ Directory tree cache refresh failed during refresh: {}",
                e
            );
        } else {
            tracing::debug!("✅ Directory tree cache ready for unified cache refresh");
        }

        // Get all projects with images for full refresh
        let projects = {
            let conn = self.db();
            let conn = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
            let db = crate::db::Database::new(&conn);
            db.get_projects_with_images()
                .map_err(|e| anyhow::anyhow!("Failed to get projects: {}", e))?
        };

        tracing::debug!(
            "🔍 Checking {} projects and their targets for file existence",
            projects.len()
        );

        let mut project_cache_updates = std::collections::HashMap::new();
        let mut target_cache_updates = std::collections::HashMap::new();
        let mut projects_with_files = 0;
        let mut targets_with_files = 0;
        let mut total_targets = 0;

        // Process all projects and their targets in one pass
        for project in &projects {
            tracing::debug!(
                "🔎 Processing project '{}' (ID: {})",
                project.name,
                project.id
            );

            // Check project files
            let project_has_files = self.check_project_files_via_cache(project.id).await?;

            if project_has_files {
                projects_with_files += 1;
            }
            project_cache_updates.insert(project.id, project_has_files);

            // Get and check all targets for this project
            let project_targets = {
                let conn = self.db();
                let conn = conn
                    .lock()
                    .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
                let db = crate::db::Database::new(&conn);
                db.get_targets_with_images(project.id).map_err(|e| {
                    anyhow::anyhow!("Failed to get targets for project {}: {}", project.id, e)
                })?
            };

            total_targets += project_targets.len();
            for (target, _, _, _) in project_targets {
                let target_has_files = self.check_target_has_files(target.id).await?;

                if target_has_files {
                    targets_with_files += 1;
                }
                target_cache_updates.insert(target.id, target_has_files);
            }
        }

        // Atomic update of both caches - hold lock for minimal time
        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache.projects_with_files = project_cache_updates;
            cache.targets_with_files = target_cache_updates;
            cache.last_updated = std::time::Instant::now();
            cache.has_initial_data = true;
        }

        let duration = start_time.elapsed();
        let total_checked = projects.len() + total_targets;
        let total_found = projects_with_files + targets_with_files;
        let total_missing = total_checked - total_found;

        tracing::info!(
            "✅ Unified cache refresh completed in {:?} - {}/{} projects have files, {}/{} targets have files",
            duration,
            projects_with_files,
            projects.len(),
            targets_with_files,
            total_targets
        );

        Ok((
            total_checked,
            total_found,
            total_missing,
            duration.as_millis(),
        ))
    }

    /// Spawn singleton background cache refresh if not already running
    /// This is the only method that should start cache refresh
    fn spawn_background_refresh(&self) -> RefreshStatus {
        // Try to atomically mark refresh as starting
        let should_start_refresh = {
            let mut cache = self.file_check_cache.write().unwrap();
            if cache.refresh_in_progress {
                // Already in progress, return appropriate status
                return if cache.has_initial_data {
                    RefreshStatus::InProgressServeStale
                } else {
                    RefreshStatus::InProgressWait
                };
            }
            // Mark as starting and continue
            cache.mark_refresh_started();
            true
        };

        if should_start_refresh {
            let state = Arc::new(self.clone());

            // Spawn the singleton refresh task
            tokio::spawn(async move {
                tracing::info!("🔄 Starting singleton cache refresh");

                // Perform the refresh
                let refresh_result = state.refresh_cache_unified_internal().await;

                // Mark refresh as completed with minimal lock time
                {
                    let mut cache = state.file_check_cache.write().unwrap();
                    cache.mark_refresh_completed();
                }

                match refresh_result {
                    Ok((checked, found, missing, duration_ms)) => {
                        tracing::info!(
                            "✅ Singleton cache refresh completed: {} checked, {} found, {} missing in {}ms", 
                            checked, found, missing, duration_ms
                        );
                    }
                    Err(e) => {
                        tracing::error!("❌ Cache refresh failed: {:?}", e);
                    }
                }
            });
        }

        // Return status based on whether we have initial data
        let cache = self.file_check_cache.read().unwrap();
        if cache.has_initial_data {
            RefreshStatus::InProgressServeStale
        } else {
            RefreshStatus::InProgressWait
        }
    }

    /// Public API method for forcing cache refresh (used by API endpoint)
    pub async fn refresh_cache_unified(
        &self,
    ) -> Result<(usize, usize, usize, u128), anyhow::Error> {
        // For API calls, we want to wait for completion, so we call the internal method directly
        self.refresh_cache_unified_internal().await
    }

    /// Check if project has files using directory tree cache
    async fn check_project_files_via_cache(&self, project_id: i32) -> Result<bool, anyhow::Error> {
        use crate::db::Database;

        let directory_tree = self.get_directory_tree().map_err(|e| {
            tracing::error!("Failed to get directory tree cache: {}", e);
            anyhow::anyhow!("Directory cache error: {}", e)
        })?;

        let all_images = {
            let conn = self.db();
            let conn = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("Database lock error"))?;
            let db = Database::new(&conn);
            db.get_images_by_project_id(project_id)
                .map_err(|e| anyhow::anyhow!("Database error: {}", e))?
        };

        if all_images.is_empty() {
            return Ok(false);
        }

        // Check if any files exist
        for (image, _project_name, _target_name) in &all_images {
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
                if let Some(filename_path) = metadata["FileName"].as_str() {
                    let filename = filename_path
                        .split(&['\\', '/'][..])
                        .next_back()
                        .unwrap_or(filename_path);

                    if directory_tree.find_file_first(filename).is_some() {
                        return Ok(true); // Early exit - found at least one file
                    }
                }
            }
        }

        Ok(false)
    }

    /// Check if target has files using directory tree cache
    async fn check_target_has_files(&self, target_id: i32) -> Result<bool, anyhow::Error> {
        use crate::db::Database;

        let directory_tree = self.get_directory_tree().map_err(|e| {
            tracing::error!("Failed to get directory tree cache: {}", e);
            anyhow::anyhow!("Directory cache error: {}", e)
        })?;

        // Get all images for this target - we need to filter by target_id
        let all_images = {
            let conn = self.db();
            let conn = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("Database lock error"))?;
            let db = Database::new(&conn);
            // Use the general query method and filter by target_id
            db.query_images(None, None, None, None)
                .map_err(|e| anyhow::anyhow!("Database error: {}", e))?
                .into_iter()
                .filter(|(img, _, _)| img.target_id == target_id)
                .collect::<Vec<_>>()
        };

        if all_images.is_empty() {
            return Ok(false);
        }

        // Check if any files exist
        for (image, _project_name, _target_name) in &all_images {
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
                if let Some(filename_path) = metadata["FileName"].as_str() {
                    let filename = filename_path
                        .split(&['\\', '/'][..])
                        .next_back()
                        .unwrap_or(filename_path);

                    if directory_tree.find_file_first(filename).is_some() {
                        return Ok(true); // Early exit - found at least one file
                    }
                }
            }
        }

        Ok(false)
    }
}

impl Clone for AppState {
    fn clone(&self) -> Self {
        Self {
            database_path: self.database_path.clone(),
            image_dirs: self.image_dirs.clone(),
            cache_dir: self.cache_dir.clone(),
            image_dir_paths: self.image_dir_paths.clone(),
            cache_dir_path: self.cache_dir_path.clone(),
            db_connection: self.db_connection.clone(),
            file_check_cache: self.file_check_cache.clone(),
            directory_tree_cache: self.directory_tree_cache.clone(),
            refresh_mutex: self.refresh_mutex.clone(),
            pregeneration_config: self.pregeneration_config.clone(),
        }
    }
}
