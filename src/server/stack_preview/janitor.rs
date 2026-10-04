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
//! names them; a variant nobody selected goes a week after its last use. An
//! artifact search goes with the stack it searched. Index files of projects
//! the catalog no longer has go too, so their stacks follow (see
//! [`drop_orphaned_indices`]); a merged project's indices move to the
//! project it merged into first. An index or selection that cannot be read
//! stops the sweep: everything it names would otherwise look unreferenced.
//!
//! Resume checkpoints are superseded in place per target/channel. One no
//! build has written or resumed from for 30 days goes, like an unused master:
//! the stack it led to is kept, and only a later build of the same group
//! would have used it. Checkpoints that can never resume again — written by
//! another pipeline version — go at once, and orphaned halves of an
//! interrupted save after a day.

use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, SystemTime};

/// Age a directory must reach before an unreferenced one is deleted. A full
/// day, so nothing a long build session or an open inspector still touches is
/// swept out from under it.
const UNREFERENCED_GRACE: Duration = Duration::from_secs(24 * 60 * 60);

/// How long an unselected stretch or deconvolution stays after it was last
/// selected or used: a person comparing variants may pick one again within
/// days.
const UNSELECTED_PROCESSING_GRACE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// How long a resume checkpoint stays after a build last wrote it or resumed
/// from it. A group that goes a month without a new frame has most likely
/// finished, and its stack does not need the checkpoint.
const CHECKPOINT_UNUSED: Duration = Duration::from_secs(30 * 24 * 60 * 60);

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

/// A checkpoint lasts until its input set changes, which replaces it in place,
/// or until no build has used it for [`CHECKPOINT_UNUSED`]. Pairs that can
/// never resume again go at once: a manifest from another pipeline version or
/// an unreadable one. An orphaned half left by an interrupted save goes after
/// the grace.
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
            // Use is the modification time: a build writes the pair and a
            // resume marks the manifest. Access times would not do, since
            // the read above moves them on a relatime mount.
            let unused = [&context, &manifest]
                .into_iter()
                .all(|path| old_enough(path, now, CHECKPOINT_UNUSED));
            if !resumable || unused {
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

/// Move a merged project's stack indices onto the project it merged into,
/// so its stacks stay referenced. Entries join the destination's own;
/// a destination entry naming the same job wins. An index that cannot be
/// read is left where it is, so nothing it names is lost.
pub fn move_project_indices(stack_root: &Path, from: i32, to: i32) {
    use crate::server::storage::{stack_folder, stack_kind, stacks};
    for (folder, prefix, list) in [
        (stacks(stack_root), "latest-project-", "groups"),
        (stacks(stack_root), "wbpp-project-", "groups"),
        (
            stack_folder(stack_root, stack_kind::COLOR),
            "latest-project-",
            "jobs",
        ),
    ] {
        let source = folder.join(format!("{prefix}{from}.json"));
        let target = folder.join(format!("{prefix}{to}.json"));
        let read = |path: &Path| -> Option<serde_json::Value> {
            serde_json::from_slice(&std::fs::read(path).ok()?).ok()
        };
        if !source.exists() {
            continue;
        }
        let Some(mut moved) = read(&source) else {
            tracing::warn!("Left {} in place: it could not be read", source.display());
            continue;
        };
        let merged = if target.exists() {
            let Some(mut kept) = read(&target) else {
                tracing::warn!(
                    "Left {} in place: {} could not be read",
                    source.display(),
                    target.display()
                );
                continue;
            };
            let known: HashSet<String> = kept[list]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|entry| entry["job_id"].as_str().map(str::to_string))
                .collect();
            let incoming: Vec<serde_json::Value> = moved[list]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| {
                    entry["job_id"]
                        .as_str()
                        .is_none_or(|job| !known.contains(job))
                })
                .collect();
            if let Some(entries) = kept[list].as_array_mut() {
                entries.extend(incoming);
            }
            kept
        } else {
            moved["project_id"] = to.into();
            moved
        };
        let staged = folder.join(format!(".{prefix}{to}.json.moving"));
        let written = serde_json::to_vec(&merged)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&staged, bytes))
            .and_then(|()| std::fs::rename(&staged, &target))
            .and_then(|()| std::fs::remove_file(&source));
        if let Err(error) = written {
            let _ = std::fs::remove_file(&staged);
            tracing::warn!(
                "Could not move {} onto project {to}: {error}",
                source.display()
            );
        }
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
        filetime::set_file_times(path, stamp, stamp).unwrap();
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
    fn a_current_checkpoint_stays_until_a_month_unused() {
        let cache = tempfile::tempdir().unwrap();
        let resume = cache.path().join("stack-previews").join("resume");
        fs::create_dir_all(&resume).unwrap();
        let pair = |stem: &str| {
            let context = resume.join(format!("{stem}.seiza-stack"));
            let manifest = resume.join(format!("{stem}.json"));
            fs::write(&context, b"x").unwrap();
            fs::write(&manifest, checkpoint_manifest("test")).unwrap();
            (context, manifest)
        };
        let (recent_context, recent_manifest) = pair("recent");
        let (resumed_context, resumed_manifest) = pair("resumed");
        let (idle_context, idle_manifest) = pair("idle");
        for path in [&recent_context, &recent_manifest] {
            age(path, 29 * 86_400);
        }
        for path in [
            &resumed_context,
            &resumed_manifest,
            &idle_context,
            &idle_manifest,
        ] {
            age(path, 31 * 86_400);
        }
        super::super::resume::mark_used(&resumed_manifest);

        // Sweep twice: the first reads every manifest, which must not make
        // a checkpoint look used to the second.
        prune(cache.path(), &keep(&[]), "test");
        prune(cache.path(), &keep(&[]), "test");

        assert!(recent_context.exists() && recent_manifest.exists());
        assert!(resumed_context.exists() && resumed_manifest.exists());
        assert!(!idle_context.exists(), "a month with no build to resume");
        assert!(!idle_manifest.exists());
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
    fn a_merged_projects_indices_join_the_project_it_merged_into() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(stacks.join("color")).unwrap();
        fs::write(
            stacks.join("latest-project-2.json"),
            r#"{"project_id":2,"groups":[{"job_id":"a"},{"job_id":"b"}]}"#,
        )
        .unwrap();
        fs::write(
            stacks.join("latest-project-1.json"),
            r#"{"project_id":1,"groups":[{"job_id":"b"},{"job_id":"c"}]}"#,
        )
        .unwrap();
        fs::write(
            stacks.join("wbpp-project-2.json"),
            r#"{"project_id":2,"groups":[{"job_id":"w"}]}"#,
        )
        .unwrap();

        move_project_indices(cache.path(), 2, 1);

        let read = |name: &str| -> serde_json::Value {
            serde_json::from_slice(&fs::read(stacks.join(name)).unwrap()).unwrap()
        };
        let jobs: Vec<String> = read("latest-project-1.json")["groups"]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["job_id"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(jobs, ["b", "c", "a"]);
        assert_eq!(read("wbpp-project-1.json")["project_id"], 1);
        assert!(!stacks.join("latest-project-2.json").exists());
        assert!(!stacks.join("wbpp-project-2.json").exists());
    }
}
