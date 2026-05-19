//! Per-database state and operations.
//!
//! Each configured database has exactly one `DatabaseContext`. It owns the
//! SQLite connection, the directory tree cache for that database's image
//! directories, the file-existence cache, and the refresh coordination
//! primitives. `AppState` holds a map of these keyed by slug.
//!
//! Methods on `DatabaseContext` are the canonical implementations for what
//! used to be on `AppState`; `AppState` keeps thin compatibility shims for
//! handlers until B2 nests them under `/api/db/{slug}/...`.

use crate::directory_tree::DirectoryTree;
use crate::server::state::{FileCheckCache, RefreshProgress, RefreshStage, RefreshStatus};
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;

/// All per-database state for the running server.
pub struct DatabaseContext {
    /// Canonical id used in URLs and on disk. URL-safe slug, e.g. `imaging-rig`
    /// or the default `db-a3f4b2c1`.
    pub id: String,
    /// Display name shown in the UI. Can be arbitrary user text.
    pub name: String,
    pub database_path: String,
    pub image_dirs: Vec<String>,
    pub image_dir_paths: Vec<PathBuf>,
    /// Cache directory for this database's generated artifacts (previews etc.).
    /// In B1 this is the shared cache root; B5 will namespace it under `<root>/<slug>`.
    pub cache_dir: String,
    pub cache_dir_path: PathBuf,
    db_connection: Arc<Mutex<Connection>>,
    pub file_check_cache: Arc<RwLock<FileCheckCache>>,
    pub directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>,
    pub refresh_mutex: Arc<TokioMutex<()>>,
}

impl DatabaseContext {
    pub fn new(
        id: String,
        name: String,
        db_path: String,
        image_dirs: Vec<String>,
        cache_dir: String,
    ) -> Result<Self> {
        use std::path::Path;

        if !Path::new(&db_path).exists() {
            return Err(anyhow::anyhow!("Database file not found: {}", db_path));
        }

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

        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        Ok(Self {
            id,
            name,
            database_path: db_path,
            image_dirs,
            image_dir_paths,
            cache_dir: cache_dir.clone(),
            cache_dir_path: PathBuf::from(cache_dir),
            db_connection: Arc::new(Mutex::new(conn)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
        })
    }

    pub fn db(&self) -> Arc<Mutex<Connection>> {
        self.db_connection.clone()
    }

    pub fn get_cache_path(&self, category: &str, filename: &str) -> PathBuf {
        self.cache_dir_path.join(category).join(filename)
    }

    pub fn get_image_path(&self, relative_path: &str) -> PathBuf {
        // First image dir for compatibility; real file lookup goes through the
        // directory tree cache for multi-directory support.
        self.image_dir_paths[0].join(relative_path)
    }

    pub fn get_directory_tree(&self) -> Result<Arc<DirectoryTree>> {
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                if !tree.is_older_than(Duration::from_secs(300)) {
                    return Ok(Arc::new(tree.clone()));
                }
            }
        }
        self.rebuild_directory_tree_internal()
    }

    fn rebuild_directory_tree_internal(&self) -> Result<Arc<DirectoryTree>> {
        tracing::debug!(
            "🌳 Building directory tree cache for db={} ({} directories, sync)",
            self.id,
            self.image_dirs.len()
        );
        let roots: Vec<&std::path::Path> =
            self.image_dir_paths.iter().map(|p| p.as_path()).collect();
        let tree = DirectoryTree::build_multiple(&roots)?;
        let stats = tree.stats();

        tracing::debug!(
            "✅ Directory tree built for db={}: {} files, {} directories across {} roots (age: {})",
            self.id,
            stats.total_files,
            stats.total_directories,
            stats.roots.len(),
            stats.format_age()
        );

        {
            let mut cache = self.directory_tree_cache.write().unwrap();
            *cache = Some(tree.clone());
        }

        Ok(Arc::new(tree))
    }

    pub fn refresh_directory_tree_if_needed(&self) -> Result<Arc<DirectoryTree>> {
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                if !tree.is_older_than(Duration::from_secs(300)) {
                    tracing::debug!(
                        "🌳 Directory tree cache is fresh for db={}, skipping rebuild",
                        self.id
                    );
                    return Ok(Arc::new(tree.clone()));
                } else {
                    tracing::debug!(
                        "🌳 Directory tree cache is stale (>5min) for db={}, rebuilding",
                        self.id
                    );
                }
            } else {
                tracing::debug!(
                    "🌳 Directory tree cache is empty for db={}, building",
                    self.id
                );
            }
        }
        self.rebuild_directory_tree_internal()
    }

    pub fn clear_directory_tree_cache(&self) {
        let mut cache = self.directory_tree_cache.write().unwrap();
        *cache = None;
        tracing::info!("🗑️  Directory tree cache cleared for db={}", self.id);
    }

    pub fn get_directory_tree_stats(&self) -> Option<crate::directory_tree::DirectoryTreeStats> {
        let cache = self.directory_tree_cache.read().unwrap();
        cache.as_ref().map(|tree| tree.stats())
    }

    pub fn ensure_cache_available(&self) -> RefreshStatus {
        let status = {
            let cache = self.file_check_cache.read().unwrap();
            cache.get_refresh_status()
        };

        match status {
            RefreshStatus::NeedsRefresh => self.spawn_background_refresh(),
            _ => status,
        }
    }

    async fn refresh_cache_unified_internal(
        &self,
    ) -> Result<(usize, usize, usize, u128), anyhow::Error> {
        let start_time = std::time::Instant::now();
        tracing::info!("🔄 Starting unified cache refresh for db={}", self.id);

        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache
                .refresh_progress
                .set_stage(RefreshStage::InitializingDirectoryTree);
            cache
                .refresh_progress
                .set_directories_info(self.image_dir_paths.len());
        }

        if let Err(e) = self.refresh_directory_tree_with_progress().await {
            tracing::warn!(
                "⚠️ Directory tree cache refresh failed during refresh: {}",
                e
            );
        } else {
            tracing::debug!("✅ Directory tree cache ready for unified cache refresh");
        }

        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache
                .refresh_progress
                .set_stage(RefreshStage::LoadingProjects);
        }

        let projects = {
            let conn = self.db();
            let conn = conn
                .lock()
                .map_err(|_| anyhow::anyhow!("Database lock failed"))?;
            let db = crate::db::Database::new(&conn);
            db.get_projects_with_images()
                .map_err(|e| anyhow::anyhow!("Failed to get projects: {}", e))?
        };

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

        for project in &projects {
            {
                let mut cache = self.file_check_cache.write().unwrap();
                cache.refresh_progress.process_project(&project.name);
            }

            tracing::debug!(
                "🔎 Processing project '{}' (ID: {})",
                project.name,
                project.id
            );

            let (project_has_files, project_files_found, project_files_missing) =
                self.check_project_files_with_details(project.id).await?;

            if project_has_files {
                projects_with_files += 1;
            }
            _total_files_found += project_files_found;
            _total_files_missing += project_files_missing;
            project_cache_updates.insert(project.id, project_has_files);

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

                {
                    let mut cache = self.file_check_cache.write().unwrap();
                    cache.refresh_progress.complete_target(
                        target_has_files,
                        target_files_found,
                        target_files_missing,
                    );
                }
            }

            {
                let mut cache = self.file_check_cache.write().unwrap();
                cache.refresh_progress.complete_project(
                    project_has_files,
                    project_files_found,
                    project_files_missing,
                );
            }
        }

        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache
                .refresh_progress
                .set_stage(RefreshStage::UpdatingCache);
        }

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
            "✅ Unified cache refresh for db={} completed in {:?} - {}/{} projects have files, {}/{} targets have files",
            self.id,
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

    fn spawn_background_refresh(&self) -> RefreshStatus {
        let should_start_refresh = {
            let mut cache = self.file_check_cache.write().unwrap();
            if cache.refresh_in_progress {
                return if cache.has_initial_data {
                    RefreshStatus::InProgressServeStale
                } else {
                    RefreshStatus::InProgressWait
                };
            }
            cache.mark_refresh_started();
            true
        };

        if should_start_refresh {
            let ctx = Arc::new(self.clone());

            tokio::spawn(async move {
                tracing::info!("🔄 Starting singleton cache refresh for db={}", ctx.id);

                let refresh_result = ctx.refresh_cache_unified_internal().await;

                {
                    let mut cache = ctx.file_check_cache.write().unwrap();
                    cache.mark_refresh_completed();
                }

                match refresh_result {
                    Ok((checked, found, missing, duration_ms)) => {
                        tracing::info!(
                            "✅ Singleton cache refresh for db={} completed: {} checked, {} found, {} missing in {}ms",
                            ctx.id, checked, found, missing, duration_ms
                        );
                    }
                    Err(e) => {
                        tracing::error!("❌ Cache refresh for db={} failed: {:?}", ctx.id, e);
                    }
                }
            });
        }

        let cache = self.file_check_cache.read().unwrap();
        if cache.has_initial_data {
            RefreshStatus::InProgressServeStale
        } else {
            RefreshStatus::InProgressWait
        }
    }

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

    async fn check_target_files_with_details(
        &self,
        target_id: i32,
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

    pub fn get_cache_refresh_progress(&self) -> Option<RefreshProgress> {
        let cache = self.file_check_cache.read().unwrap();
        if cache.refresh_in_progress {
            Some(cache.refresh_progress.clone())
        } else {
            None
        }
    }

    pub fn force_directory_tree_refresh(&self) -> RefreshStatus {
        {
            let mut dir_cache = self.directory_tree_cache.write().unwrap();
            *dir_cache = None;
            tracing::info!(
                "🗑️  Directory tree cache cleared for db={}, forcing refresh",
                self.id
            );
        }

        {
            let mut file_cache = self.file_check_cache.write().unwrap();
            file_cache.clear();
            tracing::info!(
                "🗑️  File cache cleared for db={}, forcing complete refresh",
                self.id
            );
        }

        self.ensure_cache_available()
    }

    async fn refresh_directory_tree_with_progress(&self) -> Result<Arc<DirectoryTree>> {
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                if !tree.is_older_than(Duration::from_secs(300)) {
                    tracing::debug!("🌳 Directory tree cache is fresh, skipping rebuild");
                    return Ok(Arc::new(tree.clone()));
                }
            }
        }

        tracing::info!(
            "🌳 Building directory tree cache for db={} ({} directories) with progress tracking",
            self.id,
            self.image_dir_paths.len()
        );

        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<(usize, usize, String)>();
        let file_check_cache = Arc::clone(&self.file_check_cache);

        let progress_task = tokio::spawn(async move {
            while let Some((dirs_processed, files_processed, current_directory)) =
                progress_rx.recv().await
            {
                let mut cache = file_check_cache.write().unwrap();
                cache.refresh_progress.process_directory(&current_directory);
                cache.refresh_progress.directories_processed = dirs_processed;
                cache.refresh_progress.update_files_scanned(files_processed);
            }
        });

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

        {
            let mut cache = self.file_check_cache.write().unwrap();
            cache.refresh_progress.complete_directory();
        }

        progress_task.abort();

        let stats = tree_result.stats();
        tracing::info!(
            "✅ Directory tree built for db={} with progress tracking: {} files, {} directories across {} roots (age: {})",
            self.id,
            stats.total_files,
            stats.total_directories,
            stats.roots.len(),
            stats.format_age()
        );

        {
            let mut cache = self.directory_tree_cache.write().unwrap();
            *cache = Some(tree_result.clone());
        }

        Ok(Arc::new(tree_result))
    }

    /// Construct a DatabaseContext for integration testing with a pre-opened
    /// connection. Skips filesystem validation.
    #[doc(hidden)]
    pub fn new_for_test(conn: Connection) -> Self {
        Self {
            id: "test".to_string(),
            name: "Test".to_string(),
            database_path: ":memory:".to_string(),
            image_dirs: vec![],
            image_dir_paths: vec![],
            cache_dir: "/tmp/psf-guard-test".to_string(),
            cache_dir_path: PathBuf::from("/tmp/psf-guard-test"),
            db_connection: Arc::new(Mutex::new(conn)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
        }
    }
}

impl Clone for DatabaseContext {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            name: self.name.clone(),
            database_path: self.database_path.clone(),
            image_dirs: self.image_dirs.clone(),
            image_dir_paths: self.image_dir_paths.clone(),
            cache_dir: self.cache_dir.clone(),
            cache_dir_path: self.cache_dir_path.clone(),
            db_connection: self.db_connection.clone(),
            file_check_cache: self.file_check_cache.clone(),
            directory_tree_cache: self.directory_tree_cache.clone(),
            refresh_mutex: self.refresh_mutex.clone(),
        }
    }
}
