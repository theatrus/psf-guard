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
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio::sync::Mutex as TokioMutex;

/// Flags used to (re)open every scheduler database connection. `NO_MUTEX`
/// because we serialize access ourselves with `Mutex<Connection>`; no `CREATE`
/// so a vanished path errors instead of leaving a junk empty database behind.
fn db_open_flags() -> OpenFlags {
    OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
}

/// How long a connection waits on SQLITE_BUSY before giving up. The scheduler
/// DB is rollback-journal SQLite, so a write (grade update) needs the
/// exclusive lock at commit and conflicts with any in-flight read on ANOTHER
/// connection. We create that contention OURSELVES: the background file-check
/// refresh runs on its own dedicated connection (so its slow queries never
/// hold the request mutex), and each of its streaming reads holds a shared
/// lock for the statement's duration — 30-80s per project on an SMB-mounted
/// DB. Without a busy handler, a grade written during a refresh surfaces
/// instantly as "database is locked" → HTTP 500; with one, the writer waits
/// out the current refresh statement and wins the gap between statements.
/// Sized to cover the worst observed single-query duration. (External writers
/// like N.I.N.A. on a network share get the same courtesy.)
const DB_BUSY_TIMEOUT: Duration = Duration::from_secs(60);

/// The one way to open a scheduler DB connection in the server: flags above +
/// busy timeout. Every open site (initial, both reopen paths, the refresh
/// connection) must go through here so none silently loses the busy handler.
fn open_scheduler_connection(path: &str) -> rusqlite::Result<Connection> {
    let conn = Connection::open_with_flags(path, db_open_flags())?;
    conn.busy_timeout(DB_BUSY_TIMEOUT)?;
    Ok(conn)
}

/// Identity of the on-disk database file: the `(device, inode)` pair on unix.
///
/// This is deliberately file *identity*, not file *content*. When another
/// process swaps the scheduler DB the safe, standard way — write a new file
/// and atomically `rename` it into place (what a sync, restore, or careful
/// `cp` + `mv` does) — the inode changes, and a long-lived connection to the
/// old inode would otherwise keep serving stale data or, once the old inode is
/// reclaimed, fail perpetually. Identity flips exactly on that event.
///
/// Crucially it does **not** flip on our own writes: committing a grade through
/// the open connection rewrites the file's bytes and mtime but keeps the same
/// inode. A content-based signal (mtime/size) would trip on every write and
/// force a spurious reopen after every grade — so we key on identity only.
///
/// Non-unix builds have no cheap stable inode here, so `fingerprint_path`
/// returns `None` and proactive detection is disabled; recovery there falls to
/// the reactive reopen-on-corruption path in [`DatabaseContext::with_db`].
#[derive(Clone, PartialEq, Eq, Debug)]
struct DbFingerprint {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

/// Fingerprint the file at `path` by identity, or `None` if it can't be stat'd
/// (mid-replace) or on a platform without a stable inode. `None` never compares
/// equal to a real fingerprint, but a transient stat failure won't trigger a
/// reopen against a file we can't read (see `ensure_fresh_connection`).
fn fingerprint_path(path: &str) -> Option<DbFingerprint> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::metadata(path).ok()?;
        Some(DbFingerprint {
            dev: meta.dev(),
            ino: meta.ino(),
        })
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Lock a `Mutex`, recovering the guard if a previous holder panicked. A panic
/// mid-query poisons the mutex but does **not** invalidate the `Connection`
/// itself (rusqlite holds no cross-call invariant that a panic would break), so
/// recovering is correct and avoids one bad request bricking the whole database
/// context — which `.lock().unwrap()` on a poisoned mutex would do.
fn lock_recover<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct TreeRebuildInflight {
    flag: Arc<AtomicBool>,
}

impl Drop for TreeRebuildInflight {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// Whether a rusqlite error is the specific signature of a connection whose
/// backing file was replaced with different (or non-database) bytes underneath
/// it. Deliberately narrow: only `SQLITE_CORRUPT` and `SQLITE_NOTADB`, the two
/// that never clear on their own. Transient conditions —
/// `SQLITE_BUSY`/`SQLITE_LOCKED` (contention), `SQLITE_IOERR` (a passing I/O
/// hiccup), `SQLITE_CANTOPEN` (momentary open failure), `SQLITE_SCHEMA` (a
/// benign re-plan) — are excluded so we don't reopen the world on a blip.
fn is_corruption_error(err: &rusqlite::Error) -> bool {
    use rusqlite::ffi::ErrorCode;
    matches!(
        err,
        rusqlite::Error::SqliteFailure(e, _)
            if matches!(e.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase)
    )
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
    /// File identity of `database_path` as of the currently open connection.
    /// Compared on every `db()` to detect an external replace.
    db_fingerprint: Arc<Mutex<Option<DbFingerprint>>>,
    /// Serializes reopens. No query path ever takes this lock, so a slow open
    /// held under it never stalls in-flight queries; `ensure_fresh_connection`
    /// `try_lock`s it so concurrent requests don't pile up behind a reopen.
    reopen_lock: Arc<Mutex<()>>,
    pub file_check_cache: Arc<RwLock<FileCheckCache>>,
    pub directory_tree_cache: Arc<RwLock<Option<DirectoryTree>>>,
    /// Serializes cold directory-tree builds so N concurrent requests on an
    /// empty cache share one filesystem scan instead of starting N.
    tree_build_lock: Arc<Mutex<()>>,
    /// True while a stale-revalidation/progress tree rebuild is scheduled or running.
    tree_rebuild_inflight: Arc<AtomicBool>,
    pub refresh_mutex: Arc<TokioMutex<()>>,
    /// Serializes memory-heavy on-demand plate solves within one database.
    /// A waiting duplicate re-checks the persistent cache before decoding
    /// pixels, so rapid clicks share the first completed solution.
    pub astrometry_solve_mutex: Arc<TokioMutex<()>>,
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

        let conn = open_scheduler_connection(&db_path)?;
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
            reopen_lock: Arc::new(Mutex::new(())),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            tree_build_lock: Arc::new(Mutex::new(())),
            tree_rebuild_inflight: Arc::new(AtomicBool::new(false)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            astrometry_solve_mutex: Arc::new(TokioMutex::new(())),
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
    /// `db()` already catches the common external swap proactively (an atomic
    /// `rename` changes the file's identity); this is the reactive belt to that
    /// suspenders, additionally recovering from an in-place overwrite that
    /// preserves the inode — the connection errors, we reopen, and retry. The
    /// closure may run twice, so keep it idempotent (a read, or a single
    /// transactional write).
    pub fn with_db<T>(&self, f: impl Fn(&crate::db::Database) -> Result<T>) -> Result<T> {
        self.ensure_fresh_connection();

        let run = || -> Result<T> {
            let conn = lock_recover(&self.db_connection);
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

    /// Reopen the connection if the backing file's identity changed since we
    /// opened it. One `stat` on the common unchanged path; the (possibly slow)
    /// open runs without holding the connection lock, so in-flight queries keep
    /// running against the old connection until the instant of the swap.
    fn ensure_fresh_connection(&self) {
        let current = fingerprint_path(&self.database_path);

        // Fast path: identity unchanged (or unstattable / non-unix, both `None`)
        // — nothing to do, and we never touch the connection.
        if *lock_recover(&self.db_fingerprint) == current {
            return;
        }

        // Identity differs. Serialize reopens with a lock no query path takes,
        // and `try_lock` it: if another thread is already reopening, use the
        // current connection this time (on unix the old inode stays readable)
        // and pick up the new one on a later call, rather than stalling here.
        let Ok(_reopen) = self.reopen_lock.try_lock() else {
            return;
        };

        // Re-check under the reopen lock in case another thread just finished.
        if *lock_recover(&self.db_fingerprint) == current {
            return;
        }
        // Couldn't stat the file (mid-replace): keep the old connection and
        // retry next time rather than reopening against an unreadable path.
        let Some(new_fp) = current else {
            return;
        };

        // Open outside the connection/fingerprint locks so a slow open doesn't
        // block queries.
        match open_scheduler_connection(&self.database_path) {
            Ok(new_conn) => {
                *lock_recover(&self.db_connection) = new_conn;
                *lock_recover(&self.db_fingerprint) = Some(new_fp);
                tracing::info!(
                    "🔁 Database file for db={} was replaced on disk ({}); reopened connection",
                    self.id,
                    self.database_path
                );
            }
            Err(e) => {
                tracing::error!(
                    "❌ Failed to reopen replaced database for db={} ({}): {}. Keeping existing connection; will retry.",
                    self.id,
                    self.database_path,
                    e
                );
                // Leave the fingerprint untouched so we retry on the next call.
            }
        }
    }

    /// Unconditionally reopen the connection (the reactive retry path). Holds
    /// the reopen lock across the open, but no query path takes that lock, so
    /// queries are not blocked.
    fn force_reopen(&self) {
        let _reopen = lock_recover(&self.reopen_lock);
        match open_scheduler_connection(&self.database_path) {
            Ok(new_conn) => {
                *lock_recover(&self.db_connection) = new_conn;
                *lock_recover(&self.db_fingerprint) = fingerprint_path(&self.database_path);
                tracing::info!(
                    "🔁 Reopened connection for db={} after a corruption-class error",
                    self.id
                );
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

    pub fn get_cache_path(&self, category: &str, filename: &str) -> PathBuf {
        self.cache_dir_path.join(category).join(filename)
    }

    pub fn get_image_path(&self, relative_path: &str) -> PathBuf {
        // First image dir for compatibility; real file lookup goes through the
        // directory tree cache for multi-directory support.
        self.image_dir_paths[0].join(relative_path)
    }

    /// Get the directory tree for file lookups. Never lets the request path
    /// pay for a full filesystem scan when *any* tree exists: a fresh tree is
    /// served as-is, a stale one is served immediately while one background
    /// thread revalidates (on a slow network mount a scan can take a minute —
    /// blocking requests on it starves the UI). Only a cold cache (no tree at
    /// all) blocks, and then all concurrent callers share a single scan.
    pub fn get_directory_tree(&self) -> Result<Arc<DirectoryTree>> {
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                let stale = tree.is_older_than(Duration::from_secs(300));
                let tree = Arc::new(tree.clone());
                if stale {
                    self.spawn_directory_tree_rebuild();
                }
                return Ok(tree);
            }
        }

        // Cold cache: one caller scans; the rest wait here and reuse the result.
        let _guard = lock_recover(&self.tree_build_lock);
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache {
                return Ok(Arc::new(tree.clone()));
            }
        }
        self.rebuild_directory_tree_internal()
    }

    /// Kick a deduplicated background rebuild of the directory tree. No-op if
    /// one is already running.
    fn spawn_directory_tree_rebuild(&self) {
        let Some(inflight) = self.try_mark_directory_tree_rebuild() else {
            return;
        };
        let ctx = self.clone();
        std::thread::spawn(move || {
            let _inflight = inflight;
            let _build_guard = lock_recover(ctx.tree_build_lock.as_ref());
            {
                let cache = ctx.directory_tree_cache.read().unwrap();
                if let Some(ref tree) = *cache
                    && !tree.is_older_than(Duration::from_secs(300))
                {
                    tracing::debug!(
                        "🌳 Background directory tree rebuild skipped for db={}; another scan refreshed it",
                        ctx.id
                    );
                    return;
                }
            }

            if let Err(e) = ctx.rebuild_directory_tree_internal() {
                tracing::warn!(
                    "⚠️ Background directory tree rebuild failed for db={}: {}",
                    ctx.id,
                    e
                );
            }
        });
    }

    fn try_mark_directory_tree_rebuild(&self) -> Option<TreeRebuildInflight> {
        if self.tree_rebuild_inflight.swap(true, Ordering::SeqCst) {
            None
        } else {
            Some(TreeRebuildInflight {
                flag: Arc::clone(&self.tree_rebuild_inflight),
            })
        }
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
        let _guard = lock_recover(self.tree_build_lock.as_ref());
        {
            let cache = self.directory_tree_cache.read().unwrap();
            if let Some(ref tree) = *cache
                && !tree.is_older_than(Duration::from_secs(300))
            {
                return Ok(Arc::new(tree.clone()));
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

        // All refresh queries run on a dedicated connection so their (possibly
        // slow — e.g. a network-mounted sqlite where every transaction pays
        // lock round-trips) transactions never hold the shared request
        // connection's mutex. API handlers keep answering while this walks the
        // database; two readers coexist at the SQLite level.
        let refresh_conn = open_scheduler_connection(&self.database_path)
            .map_err(|e| anyhow::anyhow!("Opening refresh connection: {}", e))?;

        let projects = {
            let db = crate::db::Database::new(&refresh_conn);
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

        let directory_tree = self.get_directory_tree().map_err(|e| {
            tracing::error!("Failed to get directory tree cache: {}", e);
            anyhow::anyhow!("Directory cache error: {}", e)
        })?;

        let mut project_cache_updates = std::collections::HashMap::new();
        let mut target_cache_updates = std::collections::HashMap::new();
        let mut projects_with_files = 0;
        let mut targets_with_files = 0;
        let mut total_targets = 0;

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

            // One query per project; per-target tallies are grouped in memory
            // so the refresh never rescans the whole image table per target.
            let images = {
                let db = crate::db::Database::new(&refresh_conn);
                db.get_images_by_project_id(project.id).map_err(|e| {
                    anyhow::anyhow!("Failed to get images for project {}: {}", project.id, e)
                })?
            };

            // target_id -> (files found, files missing)
            let mut per_target: std::collections::HashMap<i32, (usize, usize)> =
                std::collections::HashMap::new();
            for (image, _project_name, _target_name) in &images {
                let entry = per_target.entry(image.target_id).or_insert((0, 0));
                let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata)
                else {
                    continue;
                };
                let Some(filename_path) = metadata["FileName"].as_str() else {
                    continue;
                };
                let filename = filename_path
                    .split(&['\\', '/'][..])
                    .next_back()
                    .unwrap_or(filename_path);
                if directory_tree.find_file_first(filename).is_some() {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }

            let project_files_found: usize = per_target.values().map(|(f, _)| f).sum();
            let project_files_missing: usize = per_target.values().map(|(_, m)| m).sum();
            let project_has_files = project_files_found > 0;
            if project_has_files {
                projects_with_files += 1;
            }
            project_cache_updates.insert(project.id, project_has_files);

            {
                let mut cache = self.file_check_cache.write().unwrap();
                cache.refresh_progress.set_targets_info(per_target.len());
            }

            total_targets += per_target.len();
            for (target_id, (found, missing)) in per_target {
                let target_has_files = found > 0;
                if target_has_files {
                    targets_with_files += 1;
                }
                target_cache_updates.insert(target_id, target_has_files);

                {
                    let mut cache = self.file_check_cache.write().unwrap();
                    cache
                        .refresh_progress
                        .complete_target(target_has_files, found, missing);
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
            if let Some(ref tree) = *cache
                && !tree.is_older_than(Duration::from_secs(300))
            {
                tracing::debug!("🌳 Directory tree cache is fresh, skipping rebuild");
                return Ok(Arc::new(tree.clone()));
            }
        }

        let _inflight = self.try_mark_directory_tree_rebuild();

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
        let directory_tree_cache = Arc::clone(&self.directory_tree_cache);
        let tree_build_lock = Arc::clone(&self.tree_build_lock);
        let tree_result = tokio::task::spawn_blocking(move || -> Result<(DirectoryTree, bool)> {
            let _build_guard = lock_recover(tree_build_lock.as_ref());
            {
                let cache = directory_tree_cache.read().unwrap();
                if let Some(ref tree) = *cache
                    && !tree.is_older_than(Duration::from_secs(300))
                {
                    return Ok((tree.clone(), false));
                }
            }

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

            let tree = crate::directory_tree::DirectoryTree::build_multiple_with_progress(
                &roots,
                &mut progress_callback,
            )?;

            {
                let mut cache = directory_tree_cache.write().unwrap();
                *cache = Some(tree.clone());
            }

            Ok((tree, true))
        })
        .await??;

        let (tree_result, built_tree) = tree_result;

        if built_tree {
            let mut cache = self.file_check_cache.write().unwrap();
            cache.refresh_progress.complete_directory();
        } else {
            tracing::debug!(
                "🌳 Directory tree cache became fresh while refresh waited for db={}",
                self.id
            );
        }

        progress_task.abort();

        let stats = tree_result.stats();
        if built_tree {
            tracing::info!(
                "✅ Directory tree built for db={} with progress tracking: {} files, {} directories across {} roots (age: {})",
                self.id,
                stats.total_files,
                stats.total_directories,
                stats.roots.len(),
                stats.format_age()
            );
        } else {
            tracing::debug!(
                "🌳 Reusing directory tree for db={}: {} files, {} directories across {} roots (age: {})",
                self.id,
                stats.total_files,
                stats.total_directories,
                stats.roots.len(),
                stats.format_age()
            );
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
            reopen_lock: Arc::new(Mutex::new(())),
            file_check_cache: Arc::new(RwLock::new(FileCheckCache::new())),
            directory_tree_cache: Arc::new(RwLock::new(None)),
            tree_build_lock: Arc::new(Mutex::new(())),
            tree_rebuild_inflight: Arc::new(AtomicBool::new(false)),
            refresh_mutex: Arc::new(TokioMutex::new(())),
            astrometry_solve_mutex: Arc::new(TokioMutex::new(())),
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
            reopen_lock: self.reopen_lock.clone(),
            file_check_cache: self.file_check_cache.clone(),
            directory_tree_cache: self.directory_tree_cache.clone(),
            tree_build_lock: self.tree_build_lock.clone(),
            tree_rebuild_inflight: self.tree_rebuild_inflight.clone(),
            refresh_mutex: self.refresh_mutex.clone(),
            astrometry_solve_mutex: self.astrometry_solve_mutex.clone(),
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

    fn make_file_refresh_db(path: &std::path::Path) {
        let conn = Connection::open(path).unwrap();
        conn.execute_batch(
            "CREATE TABLE project (
                Id INTEGER PRIMARY KEY,
                profileId TEXT,
                name TEXT NOT NULL,
                description TEXT
            );
            CREATE TABLE target (
                Id INTEGER PRIMARY KEY,
                name TEXT NOT NULL,
                active INTEGER,
                ra REAL,
                dec REAL,
                projectid INTEGER
            );
            CREATE TABLE acquiredimage (
                Id INTEGER PRIMARY KEY,
                projectId INTEGER,
                targetId INTEGER,
                acquireddate INTEGER,
                filtername TEXT,
                gradingStatus INTEGER,
                metadata TEXT,
                rejectreason TEXT,
                profileId TEXT
            );
            INSERT INTO project (Id, profileId, name, description)
                VALUES (1, 'default', 'Project', NULL);
            INSERT INTO target (Id, name, active, ra, dec, projectid)
                VALUES
                    (10, 'Has File', 1, NULL, NULL, 1),
                    (20, 'No Filename', 1, NULL, NULL, 1),
                    (30, 'Bad Metadata', 1, NULL, NULL, 1);
            INSERT INTO acquiredimage
                (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, rejectreason, profileId)
                VALUES
                    (100, 1, 10, 1000, 'L', 0, '{\"FileName\":\"/remote/present.fit\"}', NULL, 'default'),
                    (200, 1, 20, 2000, 'L', 0, '{}', NULL, 'default'),
                    (300, 1, 30, 3000, 'L', 0, 'not json', NULL, 'default');",
        )
        .unwrap();
    }

    // Only used by the unix-only replace-by-rename test below; gated to match so
    // Windows (`-D warnings`) doesn't see it as dead code.
    #[cfg(unix)]
    fn read_project(ctx: &DatabaseContext) -> String {
        let conn = ctx.db();
        let guard = lock_recover(&conn);
        guard
            .query_row("SELECT name FROM project WHERE Id = 1", [], |r| r.get(0))
            .unwrap()
    }

    /// Plant a connection-local TEMP table on the currently open connection.
    /// TEMP tables live only for the life of one connection, so `probe_survives`
    /// returning false is a precise, deterministic signal that the connection
    /// was reopened.
    fn plant_probe(ctx: &DatabaseContext) {
        let conn = ctx.db();
        let guard = lock_recover(&conn);
        guard
            .execute_batch("CREATE TEMP TABLE _reopen_probe(x)")
            .unwrap();
    }

    fn probe_survives(ctx: &DatabaseContext) -> bool {
        let conn = ctx.db();
        let guard = lock_recover(&conn);
        let n: i64 = guard
            .query_row(
                "SELECT count(*) FROM sqlite_temp_master WHERE type='table' AND name='_reopen_probe'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        n == 1
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

    #[tokio::test]
    async fn refresh_cache_records_targets_without_usable_filename_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("sched.sqlite");
        make_file_refresh_db(&db_path);
        let image_dir = tmp.path().join("images");
        std::fs::create_dir_all(&image_dir).unwrap();
        std::fs::write(image_dir.join("present.fit"), b"fits").unwrap();
        let ctx = build_ctx(tmp.path(), &db_path);

        let (checked, found, missing, _) = ctx.refresh_cache_unified_internal().await.unwrap();

        assert_eq!((checked, found, missing), (4, 2, 2));
        let cache = ctx.file_check_cache.read().unwrap();
        assert_eq!(cache.projects_with_files.get(&1), Some(&true));
        assert_eq!(cache.targets_with_files.len(), 3);
        assert_eq!(cache.targets_with_files.get(&10), Some(&true));
        assert_eq!(cache.targets_with_files.get(&20), Some(&false));
        assert_eq!(cache.targets_with_files.get(&30), Some(&false));
    }

    // Unix-only: the proactive reopen this exercises is itself `#[cfg(unix)]`
    // (identity fingerprint needs a stable dev/ino; `fingerprint_path` returns
    // `None` elsewhere), and the setup — renaming a fresh file over one an open
    // SQLite connection still holds — is a POSIX behavior. On Windows that
    // rename fails with ERROR_ACCESS_DENIED (the default SQLite VFS opens
    // without FILE_SHARE_DELETE), so there is nothing to test there; Windows
    // relies on the reactive reopen-on-corruption path instead.
    #[cfg(unix)]
    #[test]
    fn reopens_when_db_file_is_replaced_by_rename() {
        // Simulates an external process atomically replacing the scheduler DB
        // (a sync, restore, or `mv new.sqlite sched.sqlite`): the inode changes.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("sched.sqlite");
        make_db(&db_path, "ALPHA");
        let ctx = build_ctx(tmp.path(), &db_path);

        plant_probe(&ctx);
        assert_eq!(read_project(&ctx), "ALPHA");
        assert!(probe_survives(&ctx), "no reopen should have happened yet");

        let replacement = tmp.path().join("replacement.sqlite");
        make_db(&replacement, "BRAVO");
        std::fs::rename(&replacement, &db_path).unwrap();

        // Without the reopen the old inode is still open and would keep
        // returning "ALPHA" forever; the fix must surface the new content and,
        // as proof it actually reopened, drop the connection-local probe.
        assert_eq!(read_project(&ctx), "BRAVO");
        assert!(
            !probe_survives(&ctx),
            "external replace must reopen the connection"
        );
    }

    #[test]
    fn our_own_writes_do_not_trigger_a_reopen() {
        // Regression guard: committing through our own connection rewrites the
        // file's bytes and mtime but keeps the inode, so it must NOT be mistaken
        // for an external replace. A content-based fingerprint (mtime/size)
        // would reopen after every grade write, wiping caches and spamming
        // "database was replaced" — the bug this fingerprint design avoids.
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("sched.sqlite");
        make_db(&db_path, "ALPHA");
        let ctx = build_ctx(tmp.path(), &db_path);

        plant_probe(&ctx);

        // A real write through our connection (as a grade update would do).
        {
            let conn = ctx.db();
            let guard = lock_recover(&conn);
            guard
                .execute("INSERT INTO project (Id, name) VALUES (2, 'WRITTEN')", [])
                .unwrap();
        }

        // The connection-local probe must still be there: no spurious reopen.
        assert!(
            probe_survives(&ctx),
            "our own write must not reopen the connection"
        );
        // And the write is visible, i.e. we're still on the same live DB.
        let count: i64 = {
            let conn = ctx.db();
            let guard = lock_recover(&conn);
            guard
                .query_row("SELECT count(*) FROM project", [], |r| r.get(0))
                .unwrap()
        };
        assert_eq!(count, 2);
    }

    #[test]
    fn corruption_errors_are_classified() {
        use rusqlite::ffi::{
            Error as FfiError, SQLITE_BUSY, SQLITE_CORRUPT, SQLITE_IOERR, SQLITE_NOTADB,
            SQLITE_SCHEMA,
        };

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

        // Transient / benign codes must NOT be treated as a file swap, or a
        // passing hiccup would needlessly reopen the connection.
        for transient in [SQLITE_BUSY, SQLITE_IOERR, SQLITE_SCHEMA] {
            let e = rusqlite::Error::SqliteFailure(FfiError::new(transient), None);
            assert!(
                !is_corruption_error(&e),
                "transient code {transient} must not be classified as corruption"
            );
        }

        // And detection survives being wrapped in an anyhow context chain.
        let wrapped = anyhow::Error::new(corrupt).context("querying images");
        assert!(error_is_corruption(&wrapped));
        assert!(!error_is_corruption(&anyhow::anyhow!(
            "some unrelated failure"
        )));
    }
}
