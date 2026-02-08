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
        self.rebuild_directory_tree_internal()
    }

    /// Internal method to rebuild directory tree cache (synchronous for compatibility)
    fn rebuild_directory_tree_internal(&self) -> Result<Arc<DirectoryTree>> {
        tracing::debug!(
            "🌳 Building directory tree cache for {} directories (sync)",
            self.image_dirs.len()
        );
        let roots: Vec<&std::path::Path> =
            self.image_dir_paths.iter().map(|p| p.as_path()).collect();
        let tree = DirectoryTree::build_multiple(&roots)?;
        let stats = tree.stats();

        tracing::debug!(
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
        self.rebuild_directory_tree_internal()
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

        // Update progress: Initializing directory tree
        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache
                .refresh_progress
                .set_stage(RefreshStage::InitializingDirectoryTree);
            cache
                .refresh_progress
                .set_directories_info(self.image_dir_paths.len());
        }

        // First, refresh the directory tree cache to ensure file lookups are up-to-date
        // We'll track progress during this operation
        if let Err(e) = self.refresh_directory_tree_with_progress().await {
            tracing::warn!(
                "⚠️ Directory tree cache refresh failed during refresh: {}",
                e
            );
        } else {
            tracing::debug!("✅ Directory tree cache ready for unified cache refresh");
        }

        // Update progress: Loading projects
        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache
                .refresh_progress
                .set_stage(RefreshStage::LoadingProjects);
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

        // Update progress with project count
        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache.refresh_progress.set_projects_info(projects.len());
        }

        tracing::debug!(
            "🔍 Checking {} projects and their targets for file existence",
            projects.len()
        );

        let mut project_cache_updates = std::collections::HashMap::new();
        let mut target_cache_updates = std::collections::HashMap::new();
        let mut projects_with_files = 0;
        let mut targets_with_files = 0;
        let mut total_targets = 0;
        let mut _total_files_found = 0;
        let mut _total_files_missing = 0;

        // Process all projects and their targets in one pass
        for project in &projects {
            // Update progress for current project
            {
                let mut cache = self.file_check_cache.write().unwrap();
                cache.refresh_progress.process_project(&project.name);
            }

            tracing::debug!(
                "🔎 Processing project '{}' (ID: {})",
                project.name,
                project.id
            );

            // Check project files with detailed counts
            let (project_has_files, project_files_found, project_files_missing) =
                self.check_project_files_with_details(project.id).await?;

            if project_has_files {
                projects_with_files += 1;
            }
            _total_files_found += project_files_found;
            _total_files_missing += project_files_missing;
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

            // Update targets count in progress
            {
                let mut cache = self.file_check_cache.write().unwrap();
                cache
                    .refresh_progress
                    .set_targets_info(project_targets.len());
            }

            total_targets += project_targets.len();
            for (target, _, _, _) in project_targets {
                let (target_has_files, target_files_found, target_files_missing) =
                    self.check_target_files_with_details(target.id).await?;

                if target_has_files {
                    targets_with_files += 1;
                }
                target_cache_updates.insert(target.id, target_has_files);

                // Update target progress
                {
                    let mut cache = self.file_check_cache.write().unwrap();
                    cache.refresh_progress.complete_target(
                        target_has_files,
                        target_files_found,
                        target_files_missing,
                    );
                }
            }

            // Complete project progress
            {
                let mut cache = self.file_check_cache.write().unwrap();
                cache.refresh_progress.complete_project(
                    project_has_files,
                    project_files_found,
                    project_files_missing,
                );
            }
        }

        // Update progress: Updating cache
        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache
                .refresh_progress
                .set_stage(RefreshStage::UpdatingCache);
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

    /// Check if project has files using directory tree cache with detailed counts
    async fn check_project_files_with_details(
        &self,
        project_id: i32,
    ) -> Result<(bool, usize, usize), anyhow::Error> {
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
            return Ok((false, 0, 0));
        }

        let mut files_found = 0;
        let mut files_missing = 0;
        let mut has_any_files = false;

        // Check all files and count found/missing
        for (image, _project_name, _target_name) in &all_images {
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
                if let Some(filename_path) = metadata["FileName"].as_str() {
                    let filename = filename_path
                        .split(&['\\', '/'][..])
                        .next_back()
                        .unwrap_or(filename_path);

                    if directory_tree.find_file_first(filename).is_some() {
                        files_found += 1;
                        has_any_files = true;
                    } else {
                        files_missing += 1;
                    }
                }
            }
        }

        Ok((has_any_files, files_found, files_missing))
    }

    /// Check if target has files using directory tree cache with detailed counts
    async fn check_target_files_with_details(
        &self,
        target_id: i32,
    ) -> Result<(bool, usize, usize), anyhow::Error> {
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
            return Ok((false, 0, 0));
        }

        let mut files_found = 0;
        let mut files_missing = 0;
        let mut has_any_files = false;

        // Check all files and count found/missing
        for (image, _project_name, _target_name) in &all_images {
            if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
                if let Some(filename_path) = metadata["FileName"].as_str() {
                    let filename = filename_path
                        .split(&['\\', '/'][..])
                        .next_back()
                        .unwrap_or(filename_path);

                    if directory_tree.find_file_first(filename).is_some() {
                        files_found += 1;
                        has_any_files = true;
                    } else {
                        files_missing += 1;
                    }
                }
            }
        }

        Ok((has_any_files, files_found, files_missing))
    }

    /// Get current cache refresh progress (returns None if not refreshing)
    pub fn get_cache_refresh_progress(&self) -> Option<RefreshProgress> {
        let cache = self.file_check_cache.read().unwrap();
        if cache.refresh_in_progress {
            Some(cache.refresh_progress.clone())
        } else {
            None
        }
    }

    /// Force directory tree cache refresh via singleton system (non-blocking)
    pub fn force_directory_tree_refresh(&self) -> RefreshStatus {
        // Clear both directory tree cache and file cache to force complete refresh
        {
            let mut dir_cache = self.directory_tree_cache.write().unwrap();
            *dir_cache = None;
            tracing::info!("🗑️  Directory tree cache cleared, forcing refresh");
        }

        {
            let mut file_cache = self.file_check_cache.write().unwrap();
            file_cache.clear();
            tracing::info!("🗑️  File cache cleared, forcing complete refresh");
        }

        // Start refresh via singleton system
        self.ensure_cache_available()
    }

    /// Refresh directory tree cache with progress tracking during unified cache refresh
    async fn refresh_directory_tree_with_progress(&self) -> Result<Arc<DirectoryTree>> {
        // Check if we have a valid cache first
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                // Check if cache is still valid (not too old)
                if !tree.is_older_than(Duration::from_secs(300)) {
                    // Cache is fresh, no need to rebuild
                    tracing::debug!("🌳 Directory tree cache is fresh, skipping rebuild");
                    return Ok(Arc::new(tree.clone()));
                }
            }
        }

        tracing::info!(
            "🌳 Building directory tree cache for {} directories with progress tracking",
            self.image_dir_paths.len()
        );

        // Create a channel for progress updates
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<(usize, usize, String)>();
        let file_check_cache = Arc::clone(&self.file_check_cache);

        // Spawn progress update task
        let progress_task = tokio::spawn(async move {
            while let Some((dirs_processed, files_processed, current_directory)) =
                progress_rx.recv().await
            {
                let mut cache = file_check_cache.write().unwrap();
                cache.refresh_progress.process_directory(&current_directory);

                // Update the progress with actual counts from directory walking
                cache.refresh_progress.directories_processed = dirs_processed;
                cache.refresh_progress.update_files_scanned(files_processed);
            }
        });

        // Build directory tree in blocking task with progress
        let image_dir_paths = self.image_dir_paths.clone();
        let tree_result = tokio::task::spawn_blocking(move || {
            let roots: Vec<&std::path::Path> =
                image_dir_paths.iter().map(|p| p.as_path()).collect();

            let mut progress_callback =
                |dirs_processed: usize, files_processed: usize, current_directory: &str| {
                    let _ = progress_tx.send((
                        dirs_processed,
                        files_processed,
                        current_directory.to_string(),
                    ));
                };

            crate::directory_tree::DirectoryTree::build_multiple_with_progress(
                &roots,
                &mut progress_callback,
            )
        })
        .await??;

        // Complete the last directory
        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache.refresh_progress.complete_directory();
        }

        // Wait for progress task to finish
        progress_task.abort();

        let stats = tree_result.stats();
        tracing::info!(
            "✅ Directory tree built with progress tracking: {} files, {} directories across {} roots (age: {})",
            stats.total_files,
            stats.total_directories,
            stats.roots.len(),
            stats.format_age()
        );

        // Store in cache
        {
            let mut cache = self.directory_tree_cache.write().unwrap();
            *cache = Some(tree_result.clone());
        }

        Ok(Arc::new(tree_result))
    }
}

impl AppState {
    /// Create an AppState for integration testing with a pre-opened database connection.
    /// Skips file system validation (no image dirs or cache dir needed).
    #[doc(hidden)]
    pub fn new_for_test(conn: Connection) -> Self {
        Self {
            database_path: ":memory:".to_string(),
            image_dirs: vec![],
            cache_dir: "/tmp/psf-guard-test".to_string(),
            image_dir_paths: vec![],
            cache_dir_path: std::path::PathBuf::from("/tmp/psf-guard-test"),
            db_connection: Arc::new(Mutex::new(conn)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            pregeneration_config: crate::cli::PregenerationConfig::default(),
        }
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
