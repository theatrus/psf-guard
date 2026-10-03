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
//! Stretch and deconvolution results are kept while a processing selection
//! names them; a variant nobody selected goes after a week. An artifact
//! search goes with the stack it searched. Index files of projects the
//! catalog no longer has go too, so their stacks follow (see
//! [`drop_orphaned_indices`]). An index that cannot be read stops the sweep:
//! everything it names would otherwise look unreferenced.
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

/// Age an unselected stretch or deconvolution must reach before it goes: a
/// person comparing variants may pick one again within days.
const UNSELECTED_PROCESSING_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Age a WBPP run in PSF Guard's own runs folder must reach, once a newer
/// run of the same scope exists, before it goes. Its master lights were
/// taken into the stack folder when the run finished.
const SUPERSEDED_RUN_GRACE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Everything the janitor must not delete, gathered by the caller from the
/// latest indices and the in-memory job maps.
pub(super) struct KeepSet {
    pub mono_job_ids: HashSet<String>,
    pub color_job_ids: HashSet<String>,
    pub color_input_ids: HashSet<String>,
    pub stretch_ids: HashSet<String>,
    pub deconvolution_ids: HashSet<String>,
    pub artifact_search_ids: HashSet<String>,
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
    prune_directories_older_than(root, keep, now, UNREFERENCED_GRACE)
}

fn prune_directories_older_than(
    root: &Path,
    keep: &HashSet<String>,
    now: SystemTime,
    grace: Duration,
) -> usize {
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
        if !path.is_dir() || !old_enough(&path, now, grace) {
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
pub(super) fn prune(stack_root: &Path, keep: &KeepSet, stacking_version: &str) {
    let now = SystemTime::now();
    use crate::server::storage::{stack_folder, stack_kind, stacks};
    let removed_mono = prune_directories(&stacks(stack_root), &keep.mono_job_ids, now);
    let removed_color = prune_directories(
        &stack_folder(stack_root, stack_kind::COLOR),
        &keep.color_job_ids,
        now,
    );
    let removed_inputs = prune_directories(
        &stack_folder(stack_root, stack_kind::COLOR_INPUTS),
        &keep.color_input_ids,
        now,
    );
    let removed_checkpoints = prune_checkpoints(
        &stack_folder(stack_root, stack_kind::RESUME),
        stacking_version,
        now,
    );
    let removed_reference_scores = super::reference::prune(stack_root);
    let removed_processing = prune_directories_older_than(
        &stack_folder(stack_root, stack_kind::STRETCH),
        &keep.stretch_ids,
        now,
        UNSELECTED_PROCESSING_GRACE,
    ) + prune_directories_older_than(
        &stack_folder(stack_root, stack_kind::DECONVOLUTION),
        &keep.deconvolution_ids,
        now,
        UNSELECTED_PROCESSING_GRACE,
    );
    let removed_searches = prune_directories(
        &stack_folder(stack_root, stack_kind::ARTIFACT_SEARCHES),
        &keep.artifact_search_ids,
        now,
    );
    if removed_mono
        + removed_color
        + removed_inputs
        + removed_checkpoints
        + removed_reference_scores
        + removed_processing
        + removed_searches
        > 0
    {
        tracing::info!(
            removed_mono,
            removed_color,
            removed_inputs,
            removed_checkpoints,
            removed_reference_scores,
            removed_processing,
            removed_searches,
            "Pruned superseded stack cache entries"
        );
    }
}

/// Remove the stack indices of projects the catalog no longer has (merged
/// or deleted), so the stacks they name stop being referenced. `live` is
/// every project id the catalog holds.
pub(super) fn drop_orphaned_indices(stack_root: &Path, live: &HashSet<i32>) -> usize {
    use crate::server::storage::{stack_folder, stack_kind, stacks};
    let mut removed = 0;
    for (folder, prefix) in [
        (stacks(stack_root), "latest-project-"),
        (stacks(stack_root), "wbpp-project-"),
        (
            stack_folder(stack_root, stack_kind::COLOR),
            "latest-project-",
        ),
    ] {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(project_id) = name
                .to_str()
                .and_then(|name| name.strip_prefix(prefix))
                .and_then(|rest| rest.strip_suffix(".json"))
                .and_then(|id| id.parse::<i32>().ok())
            else {
                continue;
            };
            if live.contains(&project_id) {
                continue;
            }
            match std::fs::remove_file(entry.path()) {
                Ok(()) => removed += 1,
                Err(error) => tracing::warn!(
                    "Failed to remove the stack index of gone project {project_id}: {error}"
                ),
            }
        }
    }
    removed
}

/// Remove WBPP runs in PSF Guard's own runs folder that a newer run of the
/// same scope replaced a month or more ago. Runs in a folder a person chose
/// are theirs and are never touched. `running` is the work folder of a run
/// in progress, if any.
pub(super) fn prune_wbpp_runs(stack_root: &Path, running: Option<&Path>, now: SystemTime) -> usize {
    let root = stack_root.join(crate::server::storage::WBPP_RUNS);
    let Ok(entries) = std::fs::read_dir(&root) else {
        return 0;
    };
    // `<scope>-<YYYYmmdd>-<HHMMSS>`, newest last within each scope.
    let mut by_scope: std::collections::HashMap<String, Vec<(String, std::path::PathBuf)>> =
        Default::default();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some((scope, stamp)) = split_run_name(&name) else {
            continue;
        };
        if path.is_dir() {
            by_scope
                .entry(scope.to_string())
                .or_default()
                .push((stamp.to_string(), path));
        }
    }
    let mut removed = 0;
    for runs in by_scope.values_mut() {
        runs.sort();
        runs.pop(); // the newest stays
        for (_, path) in runs.iter() {
            if running.is_some_and(|running| running.starts_with(path))
                || !old_enough(path, now, SUPERSEDED_RUN_GRACE)
            {
                continue;
            }
            match std::fs::remove_dir_all(path) {
                Ok(()) => removed += 1,
                Err(error) => tracing::warn!(
                    "Failed to remove superseded WBPP run {}: {error}",
                    path.display()
                ),
            }
        }
    }
    removed
}

/// A run folder name's scope and its `YYYYmmdd-HHMMSS` stamp.
fn split_run_name(name: &str) -> Option<(&str, &str)> {
    let split = name.len().checked_sub("-YYYYmmdd-HHMMSS".len())?;
    let (scope, stamp) = name.split_at(split);
    let stamp = stamp.strip_prefix('-')?;
    let valid = stamp.len() == 15
        && stamp.char_indices().all(|(index, c)| {
            if index == 8 {
                c == '-'
            } else {
                c.is_ascii_digit()
            }
        });
    (valid && !scope.is_empty()).then_some((scope, stamp))
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
            stretch_ids: HashSet::new(),
            deconvolution_ids: HashSet::new(),
            artifact_search_ids: HashSet::new(),
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
                stretch_ids: HashSet::new(),
                deconvolution_ids: HashSet::new(),
                artifact_search_ids: HashSet::new(),
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

    #[test]
    fn unselected_processing_stays_a_week_and_selected_processing_stays() {
        let cache = tempfile::tempdir().unwrap();
        let stretch = cache.path().join("stack-previews").join("stretch");
        let selected = stretch.join(hex_name('a'));
        let recent = stretch.join(hex_name('b'));
        let old = stretch.join(hex_name('c'));
        for path in [&selected, &recent, &old] {
            fs::create_dir_all(path).unwrap();
            age(path, 8 * 86_400);
        }
        age(&recent, 2 * 86_400);
        let mut set = keep(&[]);
        set.stretch_ids.insert(hex_name('a'));

        prune(cache.path(), &set, "test");

        assert!(selected.exists());
        assert!(recent.exists());
        assert!(!old.exists());
    }

    #[test]
    fn indices_of_projects_the_catalog_no_longer_has_go() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(stacks.join("color")).unwrap();
        for name in [
            "latest-project-1.json",
            "latest-project-2.json",
            "wbpp-project-2.json",
            "color/latest-project-2.json",
        ] {
            fs::write(stacks.join(name), "{}").unwrap();
        }

        let removed = drop_orphaned_indices(cache.path(), &[1].into_iter().collect());

        assert_eq!(removed, 3);
        assert!(stacks.join("latest-project-1.json").exists());
        assert!(!stacks.join("wbpp-project-2.json").exists());
        assert!(!stacks.join("color/latest-project-2.json").exists());
    }

    #[test]
    fn a_wbpp_run_goes_a_month_after_a_newer_one_of_its_scope() {
        let cache = tempfile::tempdir().unwrap();
        let runs = cache.path().join("wbpp");
        let make = |name: &str, days: u64| {
            let path = runs.join(name);
            fs::create_dir_all(&path).unwrap();
            age(&path, days * 86_400);
            path
        };
        let oldest = make("M42-20260101-010101", 90);
        let running = make("M42-20260201-010101", 60);
        let recent = make("M42-20260901-010101", 10);
        let newest = make("M42-20260920-010101", 40);
        let alone = make("NGC_7000-20260101-010101", 90);
        let unrelated = make("notes", 90);

        let removed = prune_wbpp_runs(
            cache.path(),
            Some(&running.join("output")),
            SystemTime::now(),
        );

        assert_eq!(removed, 1);
        assert!(!oldest.exists());
        assert!(running.exists(), "a run in progress stays");
        assert!(recent.exists(), "replaced too recently");
        assert!(newest.exists(), "the newest of a scope stays");
        assert!(alone.exists());
        assert!(unrelated.exists());
    }
}
