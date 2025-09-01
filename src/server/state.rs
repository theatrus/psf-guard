use crate::cli::PregenerationConfig;
use crate::directory_tree::DirectoryTree;
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as TokioMutex;

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
        }
    }

    pub fn is_expired(&self) -> bool {
        self.last_updated.elapsed() > self.cache_duration
    }

    pub fn clear(&mut self) {
        self.projects_with_files.clear();
        self.targets_with_files.clear();
        self.last_updated = Instant::now();
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

    /// Spawn background cache refresh if not already running
    pub fn spawn_background_refresh(&self) -> bool {
        let refresh_mutex = self.refresh_mutex.clone();
        let state = Arc::new(self.clone());

        // Spawn the task and let it try to acquire the lock
        tokio::spawn(async move {
            // Try to acquire the refresh lock without blocking
            if let Ok(guard) = refresh_mutex.try_lock() {
                tracing::info!("🔄 Starting background cache refresh");

                // Import refresh function here to avoid circular dependencies
                if let Err(e) = crate::server::handlers::refresh_project_cache(&state).await {
                    tracing::error!("❌ Background project cache refresh failed: {:?}", e);
                } else {
                    tracing::info!("✅ Background cache refresh completed");
                }

                // Guard is automatically dropped here
                drop(guard);

                // Note: We only refresh the project cache in background since it's much more expensive
                // than target cache. Target cache refresh is relatively fast and can be done on-demand.
            } else {
                tracing::debug!("🔄 Background refresh already running, skipping");
            }
        });

        // We always return true since we spawned the task (even if it might not get the lock)
        true
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
