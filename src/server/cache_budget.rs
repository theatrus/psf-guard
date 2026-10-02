//! Keeping the cache's volume from filling.
//!
//! Every database caches below the server's cache root: image previews,
//! annotated previews and star lists that any viewer can make again in a
//! moment, beside stacks, color composites and calibration masters that take
//! minutes to hours. A volume shared with other data can fill, so a person
//! sets the highest share of it that may be used. Every few minutes this
//! module reads the volume as `df` does and, when it is over that share,
//! deletes image previews least recently used first until it is back under,
//! then stack resume checkpoints a day old or more. Stacks, color previews,
//! calibration masters and WBPP masters are never culled: they are what the
//! space is for.
//!
//! Least recently used means the file's access time. Many mounts do not
//! update it on read (`noatime`, NFS), so serving a cached preview sets it
//! explicitly, at most once an hour per file.

use crate::server::state::AppState;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

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
/// How often serving a preview moves its access time.
const TOUCH_INTERVAL: Duration = Duration::from_secs(60 * 60);
/// Image previews, culled first. Each is one file below the category.
const PREVIEW_CATEGORIES: [&str; 3] = ["previews", "annotated", "stars"];

static MAX_VOLUME_PERCENT: AtomicU8 = AtomicU8::new(DEFAULT_MAX_VOLUME_PERCENT);
static OVER_LIMIT: AtomicBool = AtomicBool::new(false);
static LAST: Mutex<Vec<VolumeReport>> = Mutex::new(Vec::new());

/// Apply the registry's choice.
pub fn configure(settings: Option<&crate::db_registry::StorageSettings>) {
    let percent = settings
        .and_then(|settings| settings.max_volume_percent)
        .unwrap_or(DEFAULT_MAX_VOLUME_PERCENT)
        .clamp(MIN_MAX_VOLUME_PERCENT, 100);
    MAX_VOLUME_PERCENT.store(percent, Ordering::Relaxed);
}

pub fn max_volume_percent() -> u8 {
    MAX_VOLUME_PERCENT.load(Ordering::Relaxed)
}

/// True when the last pass left a volume over the limit with nothing more
/// it may cull, so background work that writes previews should wait.
pub fn over_limit() -> bool {
    OVER_LIMIT.load(Ordering::Relaxed)
}

/// What the last pass found on each cache volume.
pub fn last_reports() -> Vec<VolumeReport> {
    LAST.lock().unwrap().clone()
}

/// One volume holding database caches, as the last pass found it.
#[derive(Debug, Clone, Serialize)]
pub struct VolumeReport {
    /// A cache directory on the volume, for people to find it by.
    pub path: String,
    pub databases: Vec<String>,
    pub total_bytes: u64,
    pub used_bytes: u64,
    /// As `df` reports it: used over what the cache could use.
    pub used_percent: f64,
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

/// Keep a pass's findings for Settings and for the work that waits on them.
pub fn record(reports: Vec<VolumeReport>) {
    OVER_LIMIT.store(
        reports.iter().any(|report| report.over_limit),
        Ordering::Relaxed,
    );
    *LAST.lock().unwrap() = reports;
}

/// One pass over every volume: read it, cull if it is over, report.
pub fn pass(state: &AppState) -> Vec<VolumeReport> {
    let limit = max_volume_percent();
    let mut volumes: Vec<(u64, Vec<(String, PathBuf)>)> = Vec::new();
    for ctx in state.all_databases() {
        let Some(device) = device_of(&ctx.cache_dir_path) else {
            continue;
        };
        let entry = (ctx.name.clone(), ctx.cache_dir_path.clone());
        match volumes.iter_mut().find(|(known, _)| *known == device) {
            Some((_, caches)) => caches.push(entry),
            None => volumes.push((device, vec![entry])),
        }
    }
    volumes
        .into_iter()
        .filter_map(|(_, caches)| report_volume(&caches, limit))
        .collect()
}

fn report_volume(caches: &[(String, PathBuf)], limit: u8) -> Option<VolumeReport> {
    let first = &caches.first()?.1;
    let mut usage = volume_usage(first)?;
    let mut culled_files = 0;
    let mut freed_bytes = 0;
    if limit < 100 && usage.percent() > f64::from(limit) {
        let target = (f64::from(limit) - HEADROOM_PERCENT).max(0.0);
        let mut need = usage.bytes_over(target);
        let now = SystemTime::now();
        // Previews first, least recently used first; then old checkpoints.
        for tier in [
            preview_candidates(caches, now),
            checkpoint_candidates(caches, now),
        ] {
            for candidate in tier {
                if need == 0 {
                    break;
                }
                if remove(&candidate.path) {
                    culled_files += 1;
                    freed_bytes += candidate.bytes;
                    need = need.saturating_sub(candidate.bytes);
                }
            }
        }
        if culled_files > 0 {
            tracing::info!(
                "🧹 Cache volume at {:.1}% (limit {limit}%): culled {culled_files} previews and checkpoints, {} MiB",
                usage.percent(),
                freed_bytes / (1 << 20)
            );
        }
        usage = volume_usage(first).unwrap_or(usage);
    }
    let mut sizes = CacheSizes::default();
    for (_, cache) in caches {
        sizes.add(cache);
    }
    let over_limit = limit < 100 && usage.percent() > f64::from(limit);
    if over_limit {
        tracing::warn!(
            "Cache volume at {:.1}% is over the {limit}% limit with no previews left to cull",
            usage.percent()
        );
    }
    Some(VolumeReport {
        path: first.display().to_string(),
        databases: caches.iter().map(|(name, _)| name.clone()).collect(),
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

struct Candidate {
    path: PathBuf,
    bytes: u64,
    last_used: SystemTime,
}

/// Every image preview not used in the last few minutes, least recently
/// used first.
fn preview_candidates(caches: &[(String, PathBuf)], now: SystemTime) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (_, cache) in caches {
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
                    path: entry.path(),
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
fn checkpoint_candidates(caches: &[(String, PathBuf)], now: SystemTime) -> Vec<Candidate> {
    let mut candidates = Vec::new();
    for (_, cache) in caches {
        let Ok(entries) = std::fs::read_dir(cache.join("stack-previews").join("resume")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            if now
                .duration_since(modified)
                .is_ok_and(|age| age < CHECKPOINT_AGE)
            {
                continue;
            }
            candidates.push(Candidate {
                bytes: tree_bytes(&path),
                path,
                last_used: modified,
            });
        }
    }
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

#[derive(Default)]
struct CacheSizes {
    previews: u64,
    stacks: u64,
    calibration: u64,
    other: u64,
}

impl CacheSizes {
    fn add(&mut self, cache: &Path) {
        let Ok(entries) = std::fs::read_dir(cache) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let bytes = tree_bytes(&entry.path());
            match name.to_string_lossy().as_ref() {
                category if PREVIEW_CATEGORIES.contains(&category) => self.previews += bytes,
                "stack-previews" => self.stacks += bytes,
                "calibration-masters" => self.calibration += bytes,
                _ => self.other += bytes,
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
        let caches = vec![("Rig".to_string(), cache.path().to_path_buf())];
        let candidates = preview_candidates(&caches, now);
        let names = candidates
            .iter()
            .map(|candidate| {
                candidate
                    .path
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
