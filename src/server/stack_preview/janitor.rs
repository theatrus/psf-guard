//! Stack cache pruning.
//!
//! Stack artifacts are content-addressed by job id, so every rebuild writes a
//! new directory and nothing ever overwrote the old ones. The in-memory job
//! map is capped, but the disk was not: superseded FITS and preview
//! directories accumulated forever. This janitor runs after a build settles
//! and deletes what nothing references any more.
//!
//! A directory is kept when any of these still points at it:
//! - a `latest-project-*.json` index — the durable last-successful results;
//! - the in-memory job map — panels may still be polling those jobs;
//! - for color inputs, the `linear_input_id` of any kept color job.
//!
//! Everything else is deleted once it is older than a grace period, which
//! protects a directory that a concurrent build is writing this moment.
//! Resume checkpoints are superseded in place per target/channel, so they are
//! pruned purely by age: one abandoned target should not hold full-frame
//! accumulator state forever.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Age a directory must reach before an unreferenced one is deleted. Long
/// enough that a build writing artifacts right now is never swept.
const UNREFERENCED_GRACE: Duration = Duration::from_secs(60 * 60);

/// Age at which an untouched resume checkpoint is dropped. Additive rebuilds
/// refresh their checkpoint on every settle, so only abandoned targets age.
const CHECKPOINT_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Everything the janitor must not delete, gathered by the caller from the
/// latest indices and the in-memory job maps.
pub(super) struct KeepSet {
    pub mono_job_ids: HashSet<String>,
    pub color_job_ids: HashSet<String>,
    pub color_input_ids: HashSet<String>,
}

/// A stack job directory name: the lowercase hex SHA-256 the job hash writes.
/// Anything else under `stack-previews/` — `color`, `resume`, the latest
/// indices — is infrastructure, never a candidate.
fn is_job_directory_name(name: &str) -> bool {
    name.len() == 64
        && name
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
}

fn old_enough(path: &Path, now: SystemTime, age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|elapsed| elapsed >= age)
}

fn prune_directories(root: &Path, keep: &HashSet<String>, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !is_job_directory_name(name) || keep.contains(name) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() || !old_enough(&path, now, UNREFERENCED_GRACE) {
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!(
                "Failed to prune stale stack cache directory {}: {error}",
                path.display()
            ),
        }
    }
    removed
}

fn prune_checkpoints(resume_root: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(resume_root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() || !old_enough(&path, now, CHECKPOINT_MAX_AGE) {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!(
                "Failed to prune stale stack checkpoint {}: {error}",
                path.display()
            ),
        }
    }
    removed
}

/// Delete every stack artifact nothing references, and every checkpoint no
/// build has refreshed within its age limit. Errors are logged per entry; one
/// undeletable directory never stops the sweep.
pub(super) fn prune(cache_root: &Path, keep: &KeepSet) {
    let now = SystemTime::now();
    let stack_root = cache_root.join("stack-previews");
    let removed_mono = prune_directories(&stack_root, &keep.mono_job_ids, now);
    let removed_color = prune_directories(&stack_root.join("color"), &keep.color_job_ids, now);
    let removed_inputs =
        prune_directories(&stack_root.join("color-inputs"), &keep.color_input_ids, now);
    let removed_checkpoints = prune_checkpoints(&stack_root.join("resume"), now);
    if removed_mono + removed_color + removed_inputs + removed_checkpoints > 0 {
        tracing::info!(
            removed_mono,
            removed_color,
            removed_inputs,
            removed_checkpoints,
            "Pruned superseded stack cache entries"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn hex_name(fill: char) -> String {
        std::iter::repeat_n(fill, 64).collect()
    }

    fn age(path: &Path, seconds_ago: u64) {
        let stamp = filetime::FileTime::from_system_time(
            SystemTime::now() - Duration::from_secs(seconds_ago),
        );
        filetime::set_file_mtime(path, stamp).unwrap();
    }

    fn keep(mono: &[&str]) -> KeepSet {
        KeepSet {
            mono_job_ids: mono.iter().map(|id| (*id).to_string()).collect(),
            color_job_ids: HashSet::new(),
            color_input_ids: HashSet::new(),
        }
    }

    #[test]
    fn an_unreferenced_old_job_directory_is_removed() {
        let cache = tempfile::tempdir().unwrap();
        let stale = cache.path().join("stack-previews").join(hex_name('a'));
        let kept = cache.path().join("stack-previews").join(hex_name('b'));
        fs::create_dir_all(&stale).unwrap();
        fs::create_dir_all(&kept).unwrap();
        age(&stale, 2 * 60 * 60);
        age(&kept, 2 * 60 * 60);

        prune(cache.path(), &keep(&[&hex_name('b')]));

        assert!(!stale.exists(), "unreferenced directory must be swept");
        assert!(kept.exists(), "the latest index still points here");
    }

    #[test]
    fn a_fresh_directory_survives_even_when_unreferenced() {
        let cache = tempfile::tempdir().unwrap();
        let racing = cache.path().join("stack-previews").join(hex_name('c'));
        fs::create_dir_all(&racing).unwrap();

        prune(cache.path(), &keep(&[]));

        assert!(
            racing.exists(),
            "a directory inside the grace period may belong to a build in flight"
        );
    }

    #[test]
    fn infrastructure_names_are_never_candidates() {
        let cache = tempfile::tempdir().unwrap();
        let stack_root = cache.path().join("stack-previews");
        let color = stack_root.join("color");
        let resume = stack_root.join("resume");
        let latest = stack_root.join("latest-project-7.json");
        fs::create_dir_all(&color).unwrap();
        fs::create_dir_all(&resume).unwrap();
        fs::write(&latest, b"{}").unwrap();
        age(&color, 3 * 60 * 60);
        age(&resume, 3 * 60 * 60);

        prune(cache.path(), &keep(&[]));

        assert!(color.exists());
        assert!(resume.exists());
        assert!(latest.exists());
    }

    #[test]
    fn color_and_input_directories_prune_by_their_own_keep_sets() {
        let cache = tempfile::tempdir().unwrap();
        let stale_color = cache
            .path()
            .join("stack-previews")
            .join("color")
            .join(hex_name('d'));
        let kept_input = cache
            .path()
            .join("stack-previews")
            .join("color-inputs")
            .join(hex_name('e'));
        fs::create_dir_all(&stale_color).unwrap();
        fs::create_dir_all(&kept_input).unwrap();
        age(&stale_color, 2 * 60 * 60);
        age(&kept_input, 2 * 60 * 60);

        prune(
            cache.path(),
            &KeepSet {
                mono_job_ids: HashSet::new(),
                color_job_ids: HashSet::new(),
                color_input_ids: [hex_name('e')].into_iter().collect(),
            },
        );

        assert!(!stale_color.exists());
        assert!(kept_input.exists());
    }

    #[test]
    fn only_ancient_checkpoints_are_dropped() {
        let cache = tempfile::tempdir().unwrap();
        let resume = cache.path().join("stack-previews").join("resume");
        fs::create_dir_all(&resume).unwrap();
        let ancient = resume.join("old.seiza-stack");
        let recent = resume.join("new.seiza-stack");
        fs::write(&ancient, b"x").unwrap();
        fs::write(&recent, b"x").unwrap();
        age(&ancient, 40 * 24 * 60 * 60);
        age(&recent, 2 * 24 * 60 * 60);

        prune(cache.path(), &keep(&[]));

        assert!(!ancient.exists(), "an abandoned checkpoint must not linger");
        assert!(recent.exists(), "a stop from this week must stay resumable");
    }
}
