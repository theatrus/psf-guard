//! Keeping the storage volumes from filling.
//!
//! Every database keeps generated files in three folders, which may sit on
//! different volumes: the cache (image previews, annotated previews and star
//! lists that any viewer can make again in a moment), stacks, and
//! calibration masters, which take minutes to hours. A volume shared with
//! other data can fill, so a person sets the highest share of it that may be
//! used, for all three alike or one each; a volume holding more than one
//! folder uses the lowest of their limits. Every few minutes this module
//! reads each volume as `df` does and, when one is over, deletes from what
//! lives on that volume only: image previews least recently used first, then
//! stack resume checkpoints a day old or more. Stacks, color previews,
//! calibration masters and WBPP masters are never culled: they are what the
//! space is for.
//!
//! Least recently used means the file's access time. Many mounts do not
//! update it on read (`noatime`, NFS), so serving a cached preview sets it
//! explicitly, at most once an hour per file.

use crate::server::state::AppState;
use crate::server::storage::{self, StorageKind, PREVIEW_CATEGORIES};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// The limit when nobody has chosen one.
pub const DEFAULT_MAX_VOLUME_PERCENT: u8 = 90;
/// The lowest limit a person may choose: below it the cache would thrash.
pub const MIN_MAX_VOLUME_PERCENT: u8 = 50;
/// How far below the limit a cull brings the volume, so the next preview
/// does not start another.
const HEADROOM_PERCENT: f64 = 2.0;
/// A preview used this recently is on someone's screen.
const RECENT: Duration = Duration::from_secs(15 * 60);
/// Checkpoints younger than this may belong to a build about to resume.
const CHECKPOINT_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the volume is read.
const PASS_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How often serving a preview moves its access time: well inside
/// [`RECENT`], so a preview on someone's screen always reads as recent.
const TOUCH_INTERVAL: Duration = Duration::from_secs(5 * 60);
/// How long a volume's cache sizes are trusted: walking a large cache over
/// NFS is slow, and only Settings shows them.
const SIZES_TTL: Duration = Duration::from_secs(60 * 60);

/// Limits for the cache, stacks and calibration masters, in that order.
static LIMITS: [AtomicU8; 3] = [
    AtomicU8::new(DEFAULT_MAX_VOLUME_PERCENT),
    AtomicU8::new(DEFAULT_MAX_VOLUME_PERCENT),
    AtomicU8::new(DEFAULT_MAX_VOLUME_PERCENT),
];
/// Each volume's limit as the last pass worked it out, by device, so
/// pre-generation stops where that volume's cull would clear.
static VOLUME_LIMITS: std::sync::LazyLock<Mutex<HashMap<u64, u8>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));
static LAST: Mutex<Vec<VolumeReport>> = Mutex::new(Vec::new());
/// One pass at a time: two reading the same volume would each cull the
/// whole excess.
static PASS: Mutex<()> = Mutex::new(());
/// Each volume's cache sizes, by device, with when they were measured.
static SIZES: std::sync::LazyLock<Mutex<HashMap<u64, (Instant, CacheSizes)>>> =
    std::sync::LazyLock::new(|| Mutex::new(HashMap::new()));

/// Apply the registry's choice. Stacks and masters without a limit of
/// their own use the cache's.
pub fn configure(settings: Option<&crate::db_registry::StorageSettings>) {
    let clamp = |percent: u8| percent.clamp(MIN_MAX_VOLUME_PERCENT, 100);
    let cache = clamp(
        settings
            .and_then(|settings| settings.max_volume_percent)
            .unwrap_or(DEFAULT_MAX_VOLUME_PERCENT),
    );
    let own = |percent: Option<u8>| percent.map(clamp).unwrap_or(cache);
    LIMITS[StorageKind::Cache as usize].store(cache, Ordering::Relaxed);
    LIMITS[StorageKind::Stacks as usize].store(
        own(settings.and_then(|settings| settings.stack_max_volume_percent)),
        Ordering::Relaxed,
    );
    LIMITS[StorageKind::Calibration as usize].store(
        own(settings.and_then(|settings| settings.calibration_max_volume_percent)),
        Ordering::Relaxed,
    );
}

/// The cache's limit.
pub fn max_volume_percent() -> u8 {
    limit_for(StorageKind::Cache)
}

pub fn limit_for(kind: StorageKind) -> u8 {
    LIMITS[kind as usize].load(Ordering::Relaxed)
}

/// Whether background work may write previews into this cache: its volume
/// must sit below the band a cull clears, so pre-generation never makes
/// previews the next pass would delete. A preview someone opens is made
/// either way. True when there is no limit or the volume cannot be read.
pub fn room_for_previews(cache_dir: &Path) -> bool {
    let limit = device_of(cache_dir)
        .and_then(|device| VOLUME_LIMITS.lock().unwrap().get(&device).copied())
        .unwrap_or_else(max_volume_percent);
    if limit >= 100 {
        return true;
    }
    volume_usage(cache_dir).is_none_or(|usage| room_at(usage, limit))
}

/// Below the band a cull clears: the cull stops two points under the limit,
/// so writing stops there too.
fn room_at(usage: Usage, limit: u8) -> bool {
    usage.percent() < f64::from(limit) - HEADROOM_PERCENT
}

/// What the last pass found on each cache volume.
pub fn last_reports() -> Vec<VolumeReport> {
    LAST.lock().unwrap().clone()
}

/// One volume holding storage folders, as the last pass found it.
#[derive(Debug, Clone, Serialize)]
pub struct VolumeReport {
    /// A storage folder on the volume, for people to find it by.
    pub path: String,
    pub databases: Vec<String>,
    /// Which of the cache, stacks and calibration masters live here.
    pub kinds: Vec<StorageKind>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    /// As `df` reports it: used over what the cache could use.
    pub used_percent: f64,
    /// The lowest limit of the folders here.
    pub max_percent: u8,
    /// Image previews, annotated previews and star lists.
    pub preview_bytes: u64,
    /// Stacks, color previews, WBPP stacks and their checkpoints.
    pub stack_bytes: u64,
    pub calibration_bytes: u64,
    pub other_bytes: u64,
    /// What this pass culled.
    pub culled_files: u64,
    pub freed_bytes: u64,
    /// Still over the limit with nothing left that may be culled.
    pub over_limit: bool,
    pub checked_unix: i64,
}

/// Mark a cached preview as just used, so it is culled last, off the async
/// runtime: on NFS a metadata call can wait on the network.
pub fn note_served_soon(path: &Path) {
    if !cfg!(unix) {
        // Nothing is culled where volumes cannot be read.
        return;
    }
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || note_served(&path));
}

/// Mark a cached preview as just used, so it is culled last.
pub fn note_served(path: &Path) {
    let now = SystemTime::now();
    let stale = std::fs::metadata(path)
        .and_then(|metadata| metadata.accessed())
        .ok()
        .and_then(|accessed| now.duration_since(accessed).ok())
        .is_none_or(|age| age >= TOUCH_INTERVAL);
    if !stale {
        return;
    }
    // Only the access time moves: previews are identified by their content
    // and the modification time feeds the ETag.
    if let Ok(file) = std::fs::File::options().write(true).open(path) {
        let _ = file.set_times(std::fs::FileTimes::new().set_accessed(now));
    }
}

/// Read every cache volume every few minutes and cull when one is over.
pub async fn run(state: Arc<AppState>) {
    // Let startup settle first.
    tokio::time::sleep(Duration::from_secs(60)).await;
    let mut interval = tokio::time::interval(PASS_INTERVAL);
    loop {
        interval.tick().await;
        let state = Arc::clone(&state);
        let reports = tokio::task::spawn_blocking(move || pass(&state))
            .await
            .unwrap_or_default();
        record(reports);
    }
}

/// Keep a pass's findings for Settings.
pub fn record(reports: Vec<VolumeReport>) {
    *LAST.lock().unwrap() = reports;
}

/// One storage folder of one database, with the kinds that live in it.
#[derive(Debug, Clone)]
struct Folder {
    database: String,
    path: PathBuf,
    kinds: Vec<StorageKind>,
}

impl Folder {
    fn holds(&self, kind: StorageKind) -> bool {
        self.kinds.contains(&kind)
    }
}

/// Every database's storage folders, one entry per distinct folder.
fn folders(state: &AppState) -> Vec<Folder> {
    let mut folders: Vec<Folder> = Vec::new();
    for ctx in state.all_databases() {
        for (kind, path) in [
            (StorageKind::Cache, &ctx.cache_dir_path),
            (StorageKind::Stacks, &ctx.stack_root),
            (StorageKind::Calibration, &ctx.calibration_root),
        ] {
            match folders
                .iter_mut()
                .find(|folder| folder.database == ctx.name && folder.path == *path)
            {
                Some(folder) => folder.kinds.push(kind),
                None => folders.push(Folder {
                    database: ctx.name.clone(),
                    path: path.clone(),
                    kinds: vec![kind],
                }),
            }
        }
    }
    folders
}

/// One pass over every volume: read it, cull if it is over, report.
pub fn pass(state: &AppState) -> Vec<VolumeReport> {
    let _one_at_a_time = PASS.lock().unwrap_or_else(|poison| poison.into_inner());
    let mut volumes: Vec<(u64, Vec<Folder>)> = Vec::new();
    for folder in folders(state) {
        let Some(device) = device_of(&folder.path) else {
            continue;
        };
        match volumes.iter_mut().find(|(known, _)| *known == device) {
            Some((_, folders)) => folders.push(folder),
            None => volumes.push((device, vec![folder])),
        }
    }
    let limits: HashMap<u64, u8> = volumes
        .iter()
        .map(|(device, folders)| (*device, volume_limit(folders)))
        .collect();
    *VOLUME_LIMITS.lock().unwrap() = limits.clone();
    volumes
        .into_iter()
        .filter_map(|(device, folders)| report_volume(device, &folders, limits[&device]))
        .collect()
}

/// The lowest limit of the kinds on a volume.
fn volume_limit(folders: &[Folder]) -> u8 {
    folders
        .iter()
        .flat_map(|folder| folder.kinds.iter().copied())
        .map(limit_for)
        .min()
        .unwrap_or_else(max_volume_percent)
}

fn report_volume(device: u64, folders: &[Folder], limit: u8) -> Option<VolumeReport> {
    let first = &folders.first()?.path;
    let mut usage = volume_usage(first)?;
    let mut culled_files = 0;
    let mut freed_bytes = 0;
    if limit < 100 && usage.percent() > f64::from(limit) {
        let target = (f64::from(limit) - HEADROOM_PERCENT).max(0.0);
        let mut need = usage.bytes_over(target);
        let now = SystemTime::now();
        // Only what lives on this volume: previews from caches here, least
        // recently used first; then old checkpoints from stacks here.
        let caches: Vec<&Path> = folders
            .iter()
            .filter(|folder| folder.holds(StorageKind::Cache))
            .map(|folder| folder.path.as_path())
            .collect();
        let stacks: Vec<&Path> = folders
            .iter()
            .filter(|folder| folder.holds(StorageKind::Stacks))
            .map(|folder| folder.path.as_path())
            .collect();
        for tier in [
            preview_candidates(&caches, now),
            checkpoint_candidates(&stacks, now),
        ] {
            for candidate in tier {
                if need == 0 {
                    break;
                }
                // Every file of the candidate goes, even after one fails.
                let removed = candidate.paths.iter().filter(|path| remove(path)).count();
                if removed > 0 {
                    culled_files += 1;
                    freed_bytes += candidate.bytes;
                    need = need.saturating_sub(candidate.bytes);
                }
            }
        }
        if culled_files > 0 {
            tracing::info!(
                "🧹 Volume of {} at {:.1}% (limit {limit}%): culled {culled_files} previews and checkpoints, {} MiB",
                first.display(),
                usage.percent(),
                freed_bytes / (1 << 20)
            );
        }
        usage = volume_usage(first).unwrap_or(usage);
    }
    // Measured again after a cull, else at most hourly.
    let sizes = {
        let cached = SIZES
            .lock()
            .unwrap()
            .get(&device)
            .filter(|(at, _)| culled_files == 0 && at.elapsed() < SIZES_TTL)
            .map(|(_, sizes)| *sizes);
        cached.unwrap_or_else(|| {
            let mut sizes = CacheSizes::default();
            for folder in folders {
                sizes.add(&folder.path);
            }
            SIZES
                .lock()
                .unwrap()
                .insert(device, (Instant::now(), sizes));
            sizes
        })
    };
    let over_limit = limit < 100 && usage.percent() > f64::from(limit);
    if over_limit {
        tracing::warn!(
            "Volume of {} at {:.1}% is over the {limit}% limit with nothing left to cull",
            first.display(),
            usage.percent()
        );
    }
    let mut databases: Vec<String> = Vec::new();
    let mut kinds: Vec<StorageKind> = Vec::new();
    for folder in folders {
        if !databases.contains(&folder.database) {
            databases.push(folder.database.clone());
        }
        for kind in &folder.kinds {
            if !kinds.contains(kind) {
                kinds.push(*kind);
            }
        }
    }
    kinds.sort_by_key(|kind| *kind as usize);
    Some(VolumeReport {
        path: first.display().to_string(),
        databases,
        kinds,
        total_bytes: usage.total,
        used_bytes: usage.used,
        used_percent: usage.percent(),
        max_percent: limit,
        preview_bytes: sizes.previews,
        stack_bytes: sizes.stacks,
        calibration_bytes: sizes.calibration,
        other_bytes: sizes.other,
        culled_files,
        freed_bytes,
        over_limit,
        checked_unix: chrono::Utc::now().timestamp(),
    })
}

/// What a cull removes together: one preview, or a checkpoint's files.
struct Candidate {
    paths: Vec<PathBuf>,
    bytes: u64,
    last_used: SystemTime,
}

/// Every image preview not used in the last few minutes, least recently
/// used first.
fn preview_candidates(caches: &[&Path], now: SystemTime) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for cache in caches {
        for category in PREVIEW_CATEGORIES {
            let Ok(entries) = std::fs::read_dir(cache.join(category)) else {
                continue;
            };
            for entry in entries.flatten() {
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if !metadata.is_file() {
                    continue;
                }
                let last_used = metadata
                    .accessed()
                    .or_else(|_| metadata.modified())
                    .unwrap_or(SystemTime::UNIX_EPOCH);
                if now.duration_since(last_used).is_ok_and(|age| age < RECENT) {
                    continue;
                }
                candidates.push(Candidate {
                    paths: vec![entry.path()],
                    bytes: metadata.len(),
                    last_used,
                });
            }
        }
    }
    candidates.sort_by_key(|candidate| candidate.last_used);
    candidates
}

/// Stack resume checkpoints a day old or more, oldest first: they only
/// save work if a stopped build resumes, and the stack they lead to is kept.
/// A checkpoint's files (its context and its manifest) go together.
fn checkpoint_candidates(stack_roots: &[&Path], now: SystemTime) -> Vec<Candidate> {
    let mut groups: HashMap<PathBuf, Candidate> = HashMap::new();
    for stack_root in stack_roots {
        let directory = storage::stack_folder(stack_root, storage::stack_kind::RESUME);
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let stem = name.split('.').next().unwrap_or(&name).to_string();
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            let group = groups.entry(directory.join(stem)).or_insert(Candidate {
                paths: Vec::new(),
                bytes: 0,
                last_used: modified,
            });
            group.bytes += tree_bytes(&path);
            group.last_used = group.last_used.max(modified);
            group.paths.push(path);
        }
    }
    let mut candidates = groups
        .into_values()
        .filter(|group| {
            now.duration_since(group.last_used)
                .is_ok_and(|age| age >= CHECKPOINT_AGE)
        })
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.last_used);
    candidates
}

fn remove(path: &Path) -> bool {
    let removed = if path.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match removed {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!("could not cull {}: {error}", path.display());
            false
        }
    }
}

#[derive(Default, Clone, Copy)]
struct CacheSizes {
    previews: u64,
    stacks: u64,
    calibration: u64,
    other: u64,
}

impl CacheSizes {
    /// Count one storage folder, whichever kinds live in it, by the names
    /// of what it holds.
    fn add(&mut self, folder: &Path) {
        let Ok(entries) = std::fs::read_dir(folder) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let bytes = tree_bytes(&entry.path());
            let name = name.to_string_lossy();
            if PREVIEW_CATEGORIES.contains(&name.as_ref()) {
                self.previews += bytes;
            } else if StorageKind::Stacks
                .database_folders()
                .contains(&name.as_ref())
            {
                self.stacks += bytes;
            } else if StorageKind::Calibration
                .database_folders()
                .contains(&name.as_ref())
            {
                self.calibration += bytes;
            } else {
                self.other += bytes;
            }
        }
    }
}

fn tree_bytes(path: &Path) -> u64 {
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return 0;
    };
    if !metadata.is_dir() {
        return metadata.len();
    }
    std::fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| tree_bytes(&entry.path()))
                .sum()
        })
        .unwrap_or(0)
}

/// A volume's space as `df` reports it.
#[derive(Debug, Clone, Copy)]
struct Usage {
    total: u64,
    used: u64,
    /// What an unprivileged writer may still use.
    available: u64,
}

impl Usage {
    fn percent(&self) -> f64 {
        let usable = self.used + self.available;
        if usable == 0 {
            0.0
        } else {
            self.used as f64 * 100.0 / usable as f64
        }
    }

    /// Bytes to free to bring use down to `percent`.
    fn bytes_over(&self, percent: f64) -> u64 {
        let usable = (self.used + self.available) as f64;
        let allowed = usable * percent / 100.0;
        (self.used as f64 - allowed).max(0.0) as u64
    }
}

#[cfg(unix)]
fn volume_usage(path: &Path) -> Option<Usage> {
    use std::os::unix::ffi::OsStrExt;
    let c_path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    // SAFETY: a valid NUL-terminated path and a zeroed statvfs to fill.
    if unsafe { libc::statvfs(c_path.as_ptr(), &mut stats) } != 0 {
        return None;
    }
    let block = stats.f_frsize as u64;
    let total = stats.f_blocks as u64 * block;
    let free = stats.f_bfree as u64 * block;
    Some(Usage {
        total,
        used: total.saturating_sub(free),
        available: stats.f_bavail as u64 * block,
    })
}

#[cfg(not(unix))]
fn volume_usage(_path: &Path) -> Option<Usage> {
    None
}

#[cfg(unix)]
fn device_of(path: &Path) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).ok().map(|metadata| metadata.dev())
}

#[cfg(not(unix))]
fn device_of(_path: &Path) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn use_is_counted_as_df_counts_it() {
        // 100 used, 50 reserved for root, 50 available: df says 66.7%.
        let usage = Usage {
            total: 200,
            used: 100,
            available: 50,
        };
        assert!((usage.percent() - 66.666).abs() < 0.01);
        // Down to 50% of the 150 a writer can use means 75 used.
        assert_eq!(usage.bytes_over(50.0), 25);
        assert_eq!(usage.bytes_over(90.0), 0);
    }

    #[test]
    fn previews_are_culled_least_recently_used_first_and_stacks_never() {
        let cache = tempfile::tempdir().unwrap();
        let previews = cache.path().join("previews");
        let stacks = cache.path().join("stack-previews").join("a".repeat(64));
        std::fs::create_dir_all(&previews).unwrap();
        std::fs::create_dir_all(&stacks).unwrap();
        let now = SystemTime::now();
        let hours = |count: u64| now - Duration::from_secs(count * 3600);
        for (name, used) in [
            ("old.png", hours(48)),
            ("newer.png", hours(5)),
            ("now.png", now),
        ] {
            let path = previews.join(name);
            std::fs::write(&path, vec![0u8; 1000]).unwrap();
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_times(std::fs::FileTimes::new().set_accessed(used))
                .unwrap();
        }
        std::fs::write(stacks.join("group-0.fits"), vec![0u8; 5000]).unwrap();
        let caches = [cache.path()];
        let candidates = preview_candidates(&caches, now);
        let names = candidates
            .iter()
            .map(|candidate| {
                candidate.paths[0]
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        // The one on screen is spared; the stack is not a candidate at all.
        assert_eq!(names, ["old.png", "newer.png"]);
        assert!(checkpoint_candidates(&caches, now).is_empty());

        let mut sizes = CacheSizes::default();
        sizes.add(cache.path());
        assert_eq!(sizes.previews, 3000);
        assert_eq!(sizes.stacks, 5000);
    }

    fn aged(path: &Path, bytes: usize, age: Duration) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, vec![0u8; bytes]).unwrap();
        let then = SystemTime::now() - age;
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(then)
                    .set_modified(then),
            )
            .unwrap();
    }

    #[test]
    fn a_volume_culls_only_what_lives_on_it() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache/db");
        let stacks = temp.path().join("stacks/db");
        let old = Duration::from_secs(3 * 86_400);
        aged(&cache.join("previews/old.png"), 100, old);
        // A cache folder's stray checkpoint and a stack folder's stray preview
        // belong to kinds that do not live there.
        aged(&cache.join("stack-previews/resume/a.json"), 100, old);
        aged(&stacks.join("previews/stray.png"), 100, old);
        aged(&stacks.join("stack-previews/resume/b.json"), 100, old);
        aged(&stacks.join("stack-previews/job/group-0.fits"), 100, old);
        let folders = [
            Folder {
                database: "Rig".into(),
                path: cache.clone(),
                kinds: vec![StorageKind::Cache, StorageKind::Calibration],
            },
            Folder {
                database: "Rig".into(),
                path: stacks.clone(),
                kinds: vec![StorageKind::Stacks],
            },
        ];
        let device = device_of(temp.path()).unwrap();

        // A limit of 0% asks for everything that may go.
        let report = report_volume(device, &folders, 0).unwrap();

        assert_eq!(report.culled_files, 2);
        assert!(!cache.join("previews/old.png").exists());
        assert!(!stacks.join("stack-previews/resume/b.json").exists());
        assert!(cache.join("stack-previews/resume/a.json").exists());
        assert!(stacks.join("previews/stray.png").exists());
        assert!(stacks.join("stack-previews/job/group-0.fits").exists());
        assert_eq!(
            report.kinds,
            vec![
                StorageKind::Cache,
                StorageKind::Stacks,
                StorageKind::Calibration
            ]
        );
    }

    #[test]
    fn stacks_and_masters_share_the_cache_limit_unless_they_have_their_own() {
        configure(Some(&crate::db_registry::StorageSettings {
            max_volume_percent: Some(80),
            stack_max_volume_percent: Some(95),
            ..Default::default()
        }));
        assert_eq!(limit_for(StorageKind::Cache), 80);
        assert_eq!(limit_for(StorageKind::Stacks), 95);
        assert_eq!(limit_for(StorageKind::Calibration), 80);
        let folder = |kinds: Vec<StorageKind>| Folder {
            database: "Rig".into(),
            path: PathBuf::new(),
            kinds,
        };
        // A volume holding two folders uses the lower limit.
        assert_eq!(volume_limit(&[folder(vec![StorageKind::Stacks])]), 95);
        assert_eq!(
            volume_limit(&[
                folder(vec![StorageKind::Stacks]),
                folder(vec![StorageKind::Cache])
            ]),
            80
        );
        configure(None);
        assert_eq!(limit_for(StorageKind::Stacks), DEFAULT_MAX_VOLUME_PERCENT);
    }

    #[test]
    fn pre_generation_stops_where_the_cull_would_start_clearing() {
        let at = |percent: u64| Usage {
            total: 100,
            used: percent,
            available: 100 - percent,
        };
        assert!(room_at(at(80), 90));
        // 88% is inside the band the cull clears down to (two points under
        // 90%), so previews written now would only be culled again.
        assert!(!room_at(at(88), 90));
        assert!(!room_at(at(95), 90));
    }

    #[test]
    fn a_checkpoints_files_are_culled_together_once_a_day_old() {
        let cache = tempfile::tempdir().unwrap();
        let resume = cache.path().join("stack-previews").join("resume");
        std::fs::create_dir_all(&resume).unwrap();
        let old = SystemTime::now() - Duration::from_secs(3 * 86_400);
        for name in ["aaa.seiza-stack", "aaa.json", "bbb.seiza-stack", "bbb.json"] {
            let path = resume.join(name);
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            if name.starts_with("aaa") {
                std::fs::File::options()
                    .write(true)
                    .open(&path)
                    .unwrap()
                    .set_times(std::fs::FileTimes::new().set_modified(old))
                    .unwrap();
            }
        }
        let candidates = checkpoint_candidates(&[cache.path()], SystemTime::now());
        // Only the old checkpoint, both of its files in one candidate.
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].paths.len(), 2);
        assert_eq!(candidates[0].bytes, 200);
    }

    #[test]
    fn serving_a_preview_moves_only_its_access_time() {
        let cache = tempfile::tempdir().unwrap();
        let path = cache.path().join("served.png");
        std::fs::write(&path, b"png").unwrap();
        let long_ago = SystemTime::now() - Duration::from_secs(10 * 86_400);
        std::fs::File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_accessed(long_ago)
                    .set_modified(long_ago),
            )
            .unwrap();
        note_served(&path);
        let metadata = std::fs::metadata(&path).unwrap();
        let accessed = metadata.accessed().unwrap();
        assert!(SystemTime::now().duration_since(accessed).unwrap() < Duration::from_secs(60));
        let modified = metadata.modified().unwrap();
        assert!(SystemTime::now().duration_since(modified).unwrap() > Duration::from_secs(86_400));
    }
}
