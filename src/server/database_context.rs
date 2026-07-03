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
use std::time::{Duration, SystemTime};
use tokio::sync::Mutex as TokioMutex;

/// Flags used to (re)open every scheduler database connection. `NO_MUTEX`
/// because we serialize access ourselves with `Mutex<Connection>`; no `CREATE`
/// so a vanished path errors instead of leaving a junk empty database behind.
fn db_open_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// Identity of the on-disk database file, used to notice when another process
/// has replaced it (a scheduler DB sync, an rsync/`cp` over the file, a
/// restore-from-backup). A long-lived SQLite connection whose file is
/// overwritten out from under it keeps a poisoned page cache and returns
/// `SQLITE_CORRUPT` / "file is not a database" *perpetually* until the
/// connection is reopened — this fingerprint is how we detect the swap and
/// trigger that reopen.
#[derive(Clone, PartialEq, Eq, Debug)]
struct DbFingerprint {
    len: u64,
    mtime: Option<SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

/// Fingerprint the file at `path`, or `None` if it can't be stat'd right now
/// (e.g. mid-replace). A `None` never compares equal to a real fingerprint, so
/// a transient stat failure won't be mistaken for "unchanged", but it also
/// won't trigger a reopen against a file we can't read (see
/// `ensure_fresh_connection`).
fn fingerprint_path(path: &str) -> Option<DbFingerprint> {
    let meta = std::fs::metadata(path).ok()?;
    Some(DbFingerprint {
        len: meta.len(),
        mtime: meta.modified().ok(),
        #[cfg(unix)]
        dev: {
            use std::os::unix::fs::MetadataExt;
            meta.dev()
        },
        #[cfg(unix)]
        ino: {
            use std::os::unix::fs::MetadataExt;
            meta.ino()
        },
    })
}

/// Whether a rusqlite error is the kind produced by a connection whose backing
/// file was swapped or truncated underneath it — the errors that never clear
/// on their own and warrant a reopen-and-retry.
fn is_corruption_error(err: &rusqlite::Error) -> bool {
    use rusqlite::ffi::ErrorCode;
    match err {
        rusqlite::Error::SqliteFailure(e, _) => matches!(
            e.code,
            ErrorCode::DatabaseCorrupt
                | ErrorCode::NotADatabase
                | ErrorCode::SystemIoFailure
                | ErrorCode::CannotOpen
                | ErrorCode::FileLockingProtocolFailed
                | ErrorCode::SchemaChanged
        ),
        _ => false,
    }
}

/// Walk an `anyhow` error chain looking for a corruption-class rusqlite error.
/// The `Database` layer returns `anyhow::Result`, so the rusqlite error is
/// wrapped one or more levels deep.
fn error_is_corruption(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<rusqlite::Error>()
            .is_some_and(is_corruption_error)
    })
}

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
    /// Per-DB cache directory: `<cache_root>/<slug>/`. Created on construction.
    /// All preview/annotated/PSF artifacts for this database live below here,
    /// so two DBs with overlapping image IDs do not collide.
    pub cache_dir: String,
    pub cache_dir_path: PathBuf,
    db_connection: Arc<Mutex<Connection>>,
    /// Fingerprint of `database_path` as of the currently open connection.
    /// Guarded by its own mutex which is always locked *before*
    /// `db_connection` when a reopen is needed, so the two never deadlock.
    db_fingerprint: Arc<Mutex<Option<DbFingerprint>>>,
    pub file_check_cache: Arc<RwLock<FileCheckCache>>,
    pub directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>,
    pub refresh_mutex: Arc<TokioMutex<()>>,
    /// Per-DB spatial (occlusion) metrics store + scan progress; persisted
    /// under `cache_dir` as spatial_metrics.json.
    pub spatial_metrics: crate::server::spatial_scan::SharedSpatialStore,
}

impl DatabaseContext {
    /// `cache_root` is the shared parent directory; this constructor appends
    /// the slug to produce a per-DB cache subdir and creates it on disk.
    pub fn new(
        id: String,
        name: String,
        db_path: String,
        image_dirs: Vec<String>,
        cache_root: String,
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

        let cache_dir_path = PathBuf::from(&cache_root).join(&id);
        std::fs::create_dir_all(&cache_dir_path).map_err(|e| {
            anyhow::anyhow!(
                "Creating cache directory {}: {}",
                cache_dir_path.display(),
                e
            )
        })?;
        let cache_dir = cache_dir_path.to_string_lossy().into_owned();

        let conn = Connection::open_with_flags(&db_path, db_open_flags())?;
        let fingerprint = fingerprint_path(&db_path);

        Ok(Self {
            id,
            name,
            database_path: db_path,
            image_dirs,
            image_dir_paths,
            cache_dir,
            cache_dir_path,
            db_connection: Arc::new(Mutex::new(conn)),
            db_fingerprint: Arc::new(Mutex::new(fingerprint)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            spatial_metrics: Arc::new(RwLock::new(Default::default())),
        })
    }

    /// Hand out the shared connection, first reopening it if the database file
    /// has been replaced on disk since we last opened it. Every query path
    /// goes through here, so an external DB swap heals on the *next* request
    /// rather than failing forever.
    pub fn db(&self) -> Arc<Mutex<Connection>> {
        self.ensure_fresh_connection();
        self.db_connection.clone()
    }

    /// The one blessed way to run a query: it locks the connection, hands the
    /// closure a ready `Database`, and transparently reopens + retries **once**
    /// if the query fails with a corruption-class error. Callers write
    /// `ctx.with_db(|db| db.query_images(...))` instead of hand-rolling the
    /// `db().lock()` + `Database::new` dance, so the reopen policy lives in
    /// exactly one place rather than being duplicated at every call site.
    ///
    /// `db()` catches an external file swap proactively (via the fingerprint);
    /// this is the reactive belt to that suspenders — it also recovers from the
    /// rare in-place overwrite that preserves size and mtime, by reacting to
    /// the error itself. The closure may run twice, so keep it idempotent (a
    /// read, or a single transactional write).
    pub fn with_db<T>(&self, f: impl Fn(&crate::db::Database) -> Result<T>) -> Result<T> {
        self.ensure_fresh_connection();

        let run = || -> Result<T> {
            let conn = self
                .db_connection
                .lock()
                .map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
            let db = crate::db::Database::new(&conn);
            f(&db)
        };

        match run() {
            Ok(value) => Ok(value),
            Err(e) if error_is_corruption(&e) => {
                tracing::warn!(
                    "🔁 db={} query hit a corruption-class error ({}); reopening connection and retrying",
                    self.id,
                    e
                );
                self.force_reopen();
                run()
            }
            Err(e) => Err(e),
        }
    }

    /// Reopen the connection if the backing file's fingerprint changed. Cheap
    /// (one `stat`) on the common unchanged path; only takes the connection
    /// lock when an actual reopen is required.
    fn ensure_fresh_connection(&self) {
        let current = fingerprint_path(&self.database_path);

        // Fast path: compare against the fingerprint without disturbing the
        // connection. Equal (including both `None`, e.g. an in-memory test DB)
        // means nothing changed.
        {
            let stored = self.db_fingerprint.lock().unwrap();
            if *stored == current {
                return;
            }
        }

        // Something differs. Take the fingerprint lock for the whole reopen so
        // concurrent requests don't race to reopen; re-check under the lock in
        // case another thread already did it.
        let mut stored = self.db_fingerprint.lock().unwrap();
        if *stored == current {
            return;
        }

        // If we couldn't stat the file (mid-replace), don't reopen against a
        // path we can't read — leave the old connection and retry next time.
        let Some(new_fp) = current else {
            return;
        };

        match Connection::open_with_flags(&self.database_path, db_open_flags()) {
            Ok(new_conn) => {
                {
                    let mut guard = self.db_connection.lock().unwrap();
                    *guard = new_conn;
                }
                *stored = Some(new_fp);
                drop(stored);
                tracing::warn!(
                    "🔁 Database file for db={} was replaced on disk ({}); reopened connection and scheduling a cache rescan",
                    self.id,
                    self.database_path
                );
                self.invalidate_caches_after_reopen();
            }
            Err(e) => {
                tracing::error!(
                    "❌ Failed to reopen replaced database for db={} ({}): {}. Keeping existing connection; will retry.",
                    self.id,
                    self.database_path,
                    e
                );
                // Leave `stored` untouched so we retry on the next access.
            }
        }
    }

    /// Unconditionally reopen the connection (used by the reactive retry path).
    fn force_reopen(&self) {
        let mut stored = self.db_fingerprint.lock().unwrap();
        match Connection::open_with_flags(&self.database_path, db_open_flags()) {
            Ok(new_conn) => {
                {
                    let mut guard = self.db_connection.lock().unwrap();
                    *guard = new_conn;
                }
                *stored = fingerprint_path(&self.database_path);
                drop(stored);
                self.invalidate_caches_after_reopen();
            }
            Err(e) => {
                tracing::error!(
                    "❌ Forced reopen of db={} ({}) failed: {}",
                    self.id,
                    self.database_path,
                    e
                );
            }
        }
    }

    /// After a reopen, drop the in-memory caches so the next request triggers a
    /// fresh directory + file-existence scan against the new database contents.
    fn invalidate_caches_after_reopen(&self) {
        {
            let mut dir_cache = self.directory_tree_cache.write().unwrap();
            *dir_cache = None;
        }
        {
            let mut file_cache = self.file_check_cache.write().unwrap();
            file_cache.clear();
        }
        tracing::info!(
            "🗑️  Caches invalidated for db={} after connection reopen",
            self.id
        );
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
            db_fingerprint: Arc::new(Mutex::new(None)),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            spatial_metrics: Arc::new(RwLock::new(Default::default())),
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
            db_fingerprint: self.db_fingerprint.clone(),
            file_check_cache: self.file_check_cache.clone(),
            directory_tree_cache: self.directory_tree_cache.clone(),
            refresh_mutex: self.refresh_mutex.clone(),
            spatial_metrics: self.spatial_metrics.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    /// Write a tiny standalone SQLite DB holding one project row.
    fn make_db(path: &std::path::Path, project_name: &str) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch("CREATE TABLE project (Id INTEGER PRIMARY KEY, name TEXT NOT NULL);")
            .unwrap();
        conn.execute(
            "INSERT INTO project (Id, name) VALUES (1, ?1)",
            [project_name],
        )
        .unwrap();
    }

    fn read_project(ctx: &DatabaseContext) -> String {
        let conn = ctx.db();
        let guard = conn.lock().unwrap();
        guard
            .query_row("SELECT name FROM project WHERE Id = 1", [], |r| r.get(0))
            .unwrap()
    }

    fn build_ctx(dir: &std::path::Path, db_path: &std::path::Path) -> DatabaseContext {
        let img_dir = dir.join("images");
        std::fs::create_dir_all(&img_dir).unwrap();
        DatabaseContext::new(
            "test".into(),
            "Test".into(),
            db_path.to_string_lossy().into_owned(),
            vec![img_dir.to_string_lossy().into_owned()],
            dir.join("cache").to_string_lossy().into_owned(),
        )
        .unwrap()
    }

    #[test]
    fn reopens_when_db_file_is_replaced_by_rename() {
        // Simulates an external process atomically replacing the scheduler DB
        // (a sync, restore, or `mv new.sqlite sched.sqlite`): the inode changes.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("sched.sqlite");
        make_db(&db_path, "ALPHA");
        let ctx = build_ctx(tmp.path(), &db_path);

        assert_eq!(read_project(&ctx), "ALPHA");

        let replacement = tmp.path().join("replacement.sqlite");
        make_db(&replacement, "BRAVO_DIFFERENT_LENGTH");
        std::fs::rename(&replacement, &db_path).unwrap();

        // Without the reopen the old inode is still open and would keep
        // returning "ALPHA" forever; the fix must surface the new content.
        assert_eq!(read_project(&ctx), "BRAVO_DIFFERENT_LENGTH");
    }

    #[test]
    fn reopens_when_db_file_is_overwritten_in_place() {
        // Simulates `cp new.sqlite sched.sqlite` / rsync: same inode, new
        // contents and (larger) length.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("sched.sqlite");
        make_db(&db_path, "ALPHA");
        let ctx = build_ctx(tmp.path(), &db_path);
        assert_eq!(read_project(&ctx), "ALPHA");

        let replacement = tmp.path().join("replacement.sqlite");
        make_db(
            &replacement,
            "CHARLIE_A_MUCH_LONGER_PROJECT_NAME_THAN_BEFORE",
        );
        std::fs::copy(&replacement, &db_path).unwrap();

        assert_eq!(
            read_project(&ctx),
            "CHARLIE_A_MUCH_LONGER_PROJECT_NAME_THAN_BEFORE"
        );
    }

    #[test]
    fn unchanged_file_keeps_the_same_connection() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("sched.sqlite");
        make_db(&db_path, "STABLE");
        let ctx = build_ctx(tmp.path(), &db_path);

        let first = Arc::as_ptr(&ctx.db());
        let second = Arc::as_ptr(&ctx.db());
        // The Arc<Mutex<Connection>> identity is stable across accesses when
        // the file hasn't changed (no spurious reopen churn).
        assert_eq!(first, second);
        assert_eq!(read_project(&ctx), "STABLE");
    }

    #[test]
    fn corruption_errors_are_classified() {
        use rusqlite::ffi::{Error as FfiError, SQLITE_BUSY, SQLITE_CORRUPT, SQLITE_NOTADB};

        let corrupt = rusqlite::Error::SqliteFailure(
            FfiError::new(SQLITE_CORRUPT),
            Some("database disk image is malformed".to_string()),
        );
        assert!(is_corruption_error(&corrupt));

        let not_a_db = rusqlite::Error::SqliteFailure(
            FfiError::new(SQLITE_NOTADB),
            Some("file is not a database".to_string()),
        );
        assert!(is_corruption_error(&not_a_db));

        // Transient contention is NOT corruption — must not trigger a reopen.
        let busy = rusqlite::Error::SqliteFailure(FfiError::new(SQLITE_BUSY), None);
        assert!(!is_corruption_error(&busy));

        // And detection survives being wrapped in an anyhow context chain.
        let wrapped = anyhow::Error::new(corrupt).context("querying images");
        assert!(error_is_corruption(&wrapped));
        assert!(!error_is_corruption(&anyhow::anyhow!(
            "some unrelated failure"
        )));
    }
}
