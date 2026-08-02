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
//! Both indices replace entries per identity — mono per target/channel,
//! color per target/kind/palette — so a directory leaves the index only when
//! a newer build of the same identity supersedes it. Unreferenced therefore
//! means the input set changed and a newer output exists (or the job never
//! published one), and a job's output stays durable until then. A day-long
//! grace on top protects directories a build or an open inspector may still
//! be touching.
//!
//! Resume checkpoints are superseded in place per target/channel and are
//! kept until then. The only checkpoints deleted outright are those that can
//! never resume again — written by another pipeline version — and orphaned
//! halves of an interrupted save.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Age a directory must reach before an unreferenced one is deleted. A full
/// day, so nothing a long build session or an open inspector still touches is
/// swept out from under it.
const UNREFERENCED_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

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

/// A checkpoint is durable until its input set changes, which replaces it in
/// place. Deletion is reserved for pairs that can never resume again: a
/// manifest from another pipeline version, an unreadable manifest, or an
/// orphaned half left by an interrupted save.
fn prune_checkpoints(resume_root: &Path, stacking_version: &str, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(resume_root) else {
        return 0;
    };
    let mut stems: std::collections::HashMap<String, (bool, bool)> =
        std::collections::HashMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let (Some(stem), Some(extension)) = (
            path.file_stem().and_then(|value| value.to_str()),
            path.extension().and_then(|value| value.to_str()),
        ) else {
            continue;
        };
        let record = stems.entry(stem.to_string()).or_default();
        match extension {
            "seiza-stack" => record.0 = true,
            "json" => record.1 = true,
            _ => {}
        }
    }
    let mut removed = 0;
    let mut remove = |path: std::path::PathBuf| match std::fs::remove_file(&path) {
        Ok(()) => removed += 1,
        Err(error) => tracing::warn!(
            "Failed to prune stack checkpoint {}: {error}",
            path.display()
        ),
    };
    for (stem, (has_context, has_manifest)) in stems {
        let context = resume_root.join(format!("{stem}.seiza-stack"));
        let manifest = resume_root.join(format!("{stem}.json"));
        if has_context && has_manifest {
            let resumable = std::fs::read(&manifest)
                .ok()
                .and_then(|bytes| {
                    serde_json::from_slice::<super::resume::ResumeManifest>(&bytes).ok()
                })
                .is_some_and(|parsed| {
                    parsed.schema_version == super::resume::RESUME_SCHEMA_VERSION
                        && parsed.stacking_version == stacking_version
                });
            if !resumable {
                remove(context);
                remove(manifest);
            }
        } else {
            // Half a checkpoint resumes nothing. The grace covers the moment
            // between Seiza writing the context and the manifest landing.
            let orphan = if has_context { context } else { manifest };
            if old_enough(&orphan, now, UNREFERENCED_GRACE) {
                remove(orphan);
            }
        }
    }
    removed
}

/// Delete every stack artifact nothing references, and every checkpoint no
/// build has refreshed within its age limit. Errors are logged per entry; one
/// undeletable directory never stops the sweep.
pub(super) fn prune(cache_root: &Path, keep: &KeepSet, stacking_version: &str) {
    let now = SystemTime::now();
    let stack_root = cache_root.join("stack-previews");
    let removed_mono = prune_directories(&stack_root, &keep.mono_job_ids, now);
    let removed_color = prune_directories(&stack_root.join("color"), &keep.color_job_ids, now);
    let removed_inputs =
        prune_directories(&stack_root.join("color-inputs"), &keep.color_input_ids, now);
    let removed_checkpoints = prune_checkpoints(&stack_root.join("resume"), stacking_version, now);
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
        age(&stale, 25 * 60 * 60);
        age(&kept, 25 * 60 * 60);

        prune(cache.path(), &keep(&[&hex_name('b')]), "test");

        assert!(!stale.exists(), "unreferenced directory must be swept");
        assert!(kept.exists(), "the latest index still points here");
    }

    #[test]
    fn a_directory_inside_the_day_long_grace_survives_even_when_unreferenced() {
        let cache = tempfile::tempdir().unwrap();
        let racing = cache.path().join("stack-previews").join(hex_name('c'));
        fs::create_dir_all(&racing).unwrap();
        age(&racing, 20 * 60 * 60);

        prune(cache.path(), &keep(&[]), "test");

        assert!(
            racing.exists(),
            "a directory inside the grace period may still be in use"
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
        age(&color, 30 * 60 * 60);
        age(&resume, 30 * 60 * 60);

        prune(cache.path(), &keep(&[]), "test");

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
        age(&stale_color, 25 * 60 * 60);
        age(&kept_input, 25 * 60 * 60);

        prune(
            cache.path(),
            &KeepSet {
                mono_job_ids: HashSet::new(),
                color_job_ids: HashSet::new(),
                color_input_ids: [hex_name('e')].into_iter().collect(),
            },
            "test",
        );

        assert!(!stale_color.exists());
        assert!(kept_input.exists());
    }

    fn checkpoint_manifest(stacking_version: &str) -> String {
        serde_json::json!({
            "schema_version": super::super::resume::RESUME_SCHEMA_VERSION,
            "stacking_version": stacking_version,
            "target_id": 7,
            "filter_name": "Ha",
            "accepted_only": false,
            "calibration_fingerprint": "cal-1",
            "frames": [],
        })
        .to_string()
    }

    #[test]
    fn a_current_checkpoint_is_durable_regardless_of_age() {
        let cache = tempfile::tempdir().unwrap();
        let resume = cache.path().join("stack-previews").join("resume");
        fs::create_dir_all(&resume).unwrap();
        let context = resume.join("group.seiza-stack");
        let manifest = resume.join("group.json");
        fs::write(&context, b"x").unwrap();
        fs::write(&manifest, checkpoint_manifest("test")).unwrap();
        age(&context, 90 * 24 * 60 * 60);
        age(&manifest, 90 * 24 * 60 * 60);

        prune(cache.path(), &keep(&[]), "test");

        assert!(context.exists(), "durable until its input set changes");
        assert!(manifest.exists());
    }

    #[test]
    fn a_checkpoint_from_another_pipeline_version_is_dropped_at_once() {
        let cache = tempfile::tempdir().unwrap();
        let resume = cache.path().join("stack-previews").join("resume");
        fs::create_dir_all(&resume).unwrap();
        let context = resume.join("group.seiza-stack");
        let manifest = resume.join("group.json");
        fs::write(&context, b"x").unwrap();
        fs::write(&manifest, checkpoint_manifest("older")).unwrap();

        prune(cache.path(), &keep(&[]), "test");

        assert!(!context.exists(), "this checkpoint can never resume again");
        assert!(!manifest.exists());
    }

    #[test]
    fn an_orphaned_context_is_dropped_only_after_the_grace() {
        let cache = tempfile::tempdir().unwrap();
        let resume = cache.path().join("stack-previews").join("resume");
        fs::create_dir_all(&resume).unwrap();
        let fresh_orphan = resume.join("saving.seiza-stack");
        let old_orphan = resume.join("stranded.seiza-stack");
        fs::write(&fresh_orphan, b"x").unwrap();
        fs::write(&old_orphan, b"x").unwrap();
        age(&old_orphan, 25 * 60 * 60);

        prune(cache.path(), &keep(&[]), "test");

        assert!(fresh_orphan.exists(), "a save may be mid-flight");
        assert!(!old_orphan.exists(), "half a checkpoint resumes nothing");
    }
}
