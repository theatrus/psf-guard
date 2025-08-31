use crate::directory_tree::DirectoryTree;
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

pub struct AppState {
    pub database_path: String,
    pub image_dir: String,
    pub cache_dir: String,
    image_dir_path: PathBuf,
    cache_dir_path: PathBuf,
    // We'll use a connection pool or create connections as needed
    db_connection: Arc<Mutex<Connection>>,
    // Cache for file existence checks
    pub file_check_cache: Arc<RwLock<FileCheckCache>>,
    // Directory tree cache for fast file lookups
    pub directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>,
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
    pub fn new(db_path: String, image_dir: String, cache_dir: String) -> Result<Self> {
        use std::path::Path;

        // Check if database exists
        if !Path::new(&db_path).exists() {
            return Err(anyhow::anyhow!("Database file not found: {}", db_path));
        }

        // Check if image directory exists
        if !Path::new(&image_dir).exists() {
            return Err(anyhow::anyhow!("Image directory not found: {}", image_dir));
        }

        // Open database connection
        let conn = Connection::open_with_flags(
            &db_path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;

        Ok(Self {
            database_path: db_path.clone(),
            image_dir: image_dir.clone(),
            cache_dir: cache_dir.clone(),
            image_dir_path: PathBuf::from(image_dir),
            cache_dir_path: PathBuf::from(cache_dir),
            db_connection: Arc::new(Mutex::new(conn)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
        })
    }

    pub fn db(&self) -> Arc<Mutex<Connection>> {
        self.db_connection.clone()
    }

    pub fn get_cache_path(&self, category: &str, filename: &str) -> PathBuf {
        self.cache_dir_path.join(category).join(filename)
    }

    pub fn get_image_path(&self, relative_path: &str) -> PathBuf {
        self.image_dir_path.join(relative_path)
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
        tracing::info!("🌳 Building directory tree cache for: {}", self.image_dir);
        let tree = DirectoryTree::build(&self.image_dir_path)?;
        let stats = tree.stats();

        tracing::info!(
            "✅ Directory tree built: {} files, {} directories (age: {})",
            stats.total_files,
            stats.total_directories,
            stats.format_age()
        );

        // Store in cache
        {
            let mut cache = self.directory_tree_cache.write().unwrap();
            *cache = Some(tree.clone());
        }

        Ok(Arc::new(tree))
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
}
