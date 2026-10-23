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
//! project it merged into first, and a moved target's entries move to its
//! new project's indices (again in each sweep, see
//! [`rehome_moved_target_entries`]). An index or selection that cannot be read
//! stops the sweep: everything it names would otherwise look unreferenced.
//!
//! A final pass's scratch folder goes when the pass ends. One a crash left
//! behind goes after the day-long grace: no pass runs that long.
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

/// Remove what a final pass that never finished left in the scratch folder:
/// anything older than the grace, since a running pass is not that old.
fn prune_scratch(scratch: &Path, now: SystemTime) -> usize {
    let Ok(entries) = std::fs::read_dir(scratch) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !old_enough(&path, now, UNREFERENCED_GRACE) {
            continue;
        }
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        match result {
            Ok(()) => removed += 1,
            Err(error) => tracing::warn!(
                "Failed to remove stale final-pass scratch {}: {error}",
                path.display()
            ),
        }
    }
    removed
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
    let removed_scratch = prune_scratch(&crate::server::storage::stack_scratch(stack_root), now);
    if removed_mono
        + removed_color
        + removed_inputs
        + removed_checkpoints
        + removed_reference_scores
        + removed_processing
        + removed_searches
        + removed_scratch
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
            removed_scratch,
            "Pruned superseded stack cache entries"
        );
    }
}

/// Remove one project's stack indices. The stacks they name are pruned once
/// nothing else references them.
pub(super) fn drop_project_indices(stack_root: &Path, project_id: i32) -> usize {
    use crate::server::storage::{stack_folder, stack_kind, stacks};
    [
        (stacks(stack_root), "latest-project-"),
        (stacks(stack_root), "wbpp-project-"),
        (
            stack_folder(stack_root, stack_kind::COLOR),
            "latest-project-",
        ),
    ]
    .into_iter()
    .filter(|(folder, prefix)| {
        std::fs::remove_file(folder.join(format!("{prefix}{project_id}.json"))).is_ok()
    })
    .count()
}

/// Remove the stack indices of projects the catalog no longer has (merged
/// or deleted), so the stacks they name stop being referenced. `live` is
/// every project id the catalog holds.
pub(super) fn drop_orphaned_indices(stack_root: &Path, live: &HashSet<i32>) -> usize {
    super::with_index_lock(stack_root, || {
        drop_orphaned_indices_locked(stack_root, live)
    })
}

fn drop_orphaned_indices_locked(stack_root: &Path, live: &HashSet<i32>) -> usize {
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
/// when both list the same job, the index keeps the destination's entry.
/// An index that cannot be read is left where it is, so nothing it names is
/// lost.
pub fn move_project_indices(stack_root: &Path, from: i32, to: i32) {
    super::with_index_lock(stack_root, || {
        move_project_indices_locked(stack_root, from, to)
    });
}

fn move_project_indices_locked(stack_root: &Path, from: i32, to: i32) {
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

/// The three kinds of stack index: folder, file prefix, entry list, and how
/// an entry identifies its target.
fn index_kinds(
    stack_root: &Path,
) -> [(std::path::PathBuf, &'static str, &'static str, TargetOf); 3] {
    use crate::server::storage::{stack_folder, stack_kind, stacks};
    [
        (
            stacks(stack_root),
            "latest-project-",
            "groups",
            group_target,
        ),
        (stacks(stack_root), "wbpp-project-", "groups", group_target),
        (
            stack_folder(stack_root, stack_kind::COLOR),
            "latest-project-",
            "jobs",
            color_target,
        ),
    ]
}

type TargetOf = fn(&serde_json::Value) -> Option<i64>;

/// A mono or WBPP entry's target.
fn group_target(entry: &serde_json::Value) -> Option<i64> {
    entry["group"]["target_id"].as_i64()
}

/// A color entry's target.
fn color_target(entry: &serde_json::Value) -> Option<i64> {
    entry["target_id"].as_i64()
}

/// The target name an entry recorded when it was built.
fn entry_target_name(entry: &serde_json::Value) -> Option<&str> {
    entry["group"]["target_name"]
        .as_str()
        .or_else(|| entry["target_name"].as_str())
}

/// Whether two target names are the same, ignoring case and surrounding
/// spaces.
fn same_target_name(left: &str, right: &str) -> bool {
    left.trim().to_lowercase() == right.trim().to_lowercase()
}

/// The identity the index writers replace an entry by: for a mono or WBPP
/// stack its target, filter, exposure group and WBPP exposure; for a color
/// preview its target, kind, palette and source family.
fn entry_identity(entry: &serde_json::Value) -> String {
    if entry.get("group").is_some() {
        serde_json::json!([
            entry["group"]["target_id"],
            entry["group"]["filter_name"],
            entry["group"]["exposure_group"]["key"],
            entry["wbpp"]["exposure_seconds"],
        ])
        .to_string()
    } else {
        serde_json::json!([
            entry["target_id"],
            entry["kind"],
            entry["palette"],
            entry["source_family_key"],
        ])
        .to_string()
    }
}

/// The project ID in an index file name, such as 7 in `latest-project-7.json`.
fn index_project(name: &str, prefix: &str) -> Option<i32> {
    name.strip_prefix(prefix)?
        .strip_suffix(".json")?
        .parse()
        .ok()
}

/// Move one target's entries from a project's stack indices to another
/// project's, after the target itself moved there. Each entry joins the
/// destination's list. When the destination already has an entry of the same
/// identity (see [`entry_identity`]), the index keeps the one built later.
/// An index that cannot be read is left as it is. Returns how many entries
/// left the source.
pub fn move_target_indices(stack_root: &Path, target_id: i32, from: i32, to: i32) -> usize {
    super::with_index_lock(stack_root, || {
        move_target_indices_locked(stack_root, target_id, from, to)
    })
}

fn move_target_indices_locked(stack_root: &Path, target_id: i32, from: i32, to: i32) -> usize {
    use serde_json::Value;
    let read =
        |path: &Path| -> Option<Value> { serde_json::from_slice(&std::fs::read(path).ok()?).ok() };
    let write = |folder: &Path, name: &str, value: &Value| -> std::io::Result<()> {
        let staged = folder.join(format!(".{name}.moving"));
        let written = serde_json::to_vec(value)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&staged, bytes))
            .and_then(|()| std::fs::rename(&staged, folder.join(name)));
        if written.is_err() {
            let _ = std::fs::remove_file(&staged);
        }
        written
    };
    let mut moved_entries = 0;
    for (folder, prefix, list, target_of) in index_kinds(stack_root) {
        let source_name = format!("{prefix}{from}.json");
        let destination_name = format!("{prefix}{to}.json");
        let source_path = folder.join(&source_name);
        let destination_path = folder.join(&destination_name);
        if !source_path.exists() {
            continue;
        }
        let Some(mut source) = read(&source_path) else {
            tracing::warn!(
                "Left {} as it is: it could not be read",
                source_path.display()
            );
            continue;
        };
        let Some(entries) = source[list].as_array().cloned() else {
            continue;
        };
        let (mut moving, kept): (Vec<Value>, Vec<Value>) = entries
            .into_iter()
            .partition(|entry| target_of(entry) == Some(i64::from(target_id)));
        if moving.is_empty() {
            continue;
        }
        for entry in &mut moving {
            // A color job records its project too.
            if let Some(project) = entry.get_mut("project_id") {
                *project = to.into();
            }
        }
        let mut destination = if destination_path.exists() {
            let Some(destination) = read(&destination_path) else {
                tracing::warn!(
                    "Left the stacks of target {target_id} in {}: {} could not be read",
                    source_path.display(),
                    destination_path.display()
                );
                continue;
            };
            destination
        } else {
            let mut empty = source.clone();
            empty["project_id"] = to.into();
            empty[list] = Value::Array(Vec::new());
            empty
        };
        if !destination[list].is_array() {
            destination[list] = Value::Array(Vec::new());
        }
        let count = moving.len();
        if let Some(entries) = destination[list].as_array_mut() {
            for entry in moving {
                let identity = entry_identity(&entry);
                let built = |entry: &Value| entry["created_unix_seconds"].as_i64().unwrap_or(0);
                match entries
                    .iter_mut()
                    .find(|existing| entry_identity(existing) == identity)
                {
                    Some(existing) if built(&entry) > built(existing) => *existing = entry,
                    Some(_) => {}
                    None => entries.push(entry),
                }
            }
        }
        // The destination first: should the source then fail to write, its
        // entries are listed twice rather than lost.
        if let Err(error) = write(&folder, &destination_name, &destination) {
            tracing::warn!(
                "Could not move the stacks of target {target_id} to {}: {error}",
                destination_path.display()
            );
            continue;
        }
        source[list] = Value::Array(kept);
        if let Err(error) = write(&folder, &source_name, &source) {
            tracing::warn!(
                "Moved the stacks of target {target_id} to {} but could not remove them from {}: {error}",
                destination_path.display(),
                source_path.display()
            );
        }
        moved_entries += count;
    }
    moved_entries
}

/// Remove a merged-away target's entries from its project's stack indices.
/// The target no longer exists, so its stacks have no card to show, and
/// the catalog can give its ID to a new target later. Returns how many
/// entries were removed.
pub fn drop_target_entries(stack_root: &Path, project: i32, target_id: i32) -> usize {
    super::with_index_lock(stack_root, || {
        let mut removed = 0;
        for (folder, prefix, list, target_of) in index_kinds(stack_root) {
            let path = folder.join(format!("{prefix}{project}.json"));
            let Some(mut index) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                continue;
            };
            let Some(entries) = index[list].as_array_mut() else {
                continue;
            };
            let before = entries.len();
            entries.retain(|entry| target_of(entry) != Some(i64::from(target_id)));
            let dropped = before - entries.len();
            if dropped == 0 {
                continue;
            }
            let staged = folder.join(format!(".{prefix}{project}.json.dropping"));
            let written = serde_json::to_vec(&index)
                .map_err(std::io::Error::other)
                .and_then(|bytes| std::fs::write(&staged, bytes))
                .and_then(|()| std::fs::rename(&staged, &path));
            match written {
                Ok(()) => removed += dropped,
                Err(error) => {
                    let _ = std::fs::remove_file(&staged);
                    tracing::warn!(
                        "Could not remove the stacks of merged target {target_id} from {}: {error}",
                        path.display()
                    );
                }
            }
        }
        removed
    })
}

/// Move each index entry whose target now belongs to another project to
/// that project's indices. A target move does this at once; the sweep does
/// it again for an entry a build wrote to the old project's index while the
/// target moved. `targets` maps each target the catalog has to its project
/// and name. An entry moves only when the name it recorded is the target's
/// current name: the catalog can give a deleted target's ID to a new
/// target, and the old target's stacks must not move to the new one's
/// project. Entries of other targets stay where they are. Returns how many
/// entries moved.
pub(super) fn rehome_moved_target_entries(
    stack_root: &Path,
    targets: &std::collections::HashMap<i32, (i32, String)>,
) -> usize {
    let mut moves = std::collections::BTreeSet::new();
    for (folder, prefix, list, target_of) in index_kinds(stack_root) {
        let Ok(files) = std::fs::read_dir(&folder) else {
            continue;
        };
        for file in files.flatten() {
            let name = file.file_name();
            let Some(project) = name.to_str().and_then(|name| index_project(name, prefix)) else {
                continue;
            };
            let Some(index) = std::fs::read(file.path())
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            else {
                continue;
            };
            for entry in index[list].as_array().into_iter().flatten() {
                let Some(target) = target_of(entry).and_then(|id| i32::try_from(id).ok()) else {
                    continue;
                };
                if let Some((current, name)) = targets.get(&target)
                    && *current != project
                    && entry_target_name(entry)
                        .is_some_and(|recorded| same_target_name(recorded, name))
                {
                    moves.insert((target, project, *current));
                }
            }
        }
    }
    moves
        .into_iter()
        .map(|(target, from, to)| move_target_indices(stack_root, target, from, to))
        .sum()
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
    fn final_pass_scratch_goes_only_once_no_pass_can_still_be_using_it() {
        let cache = tempfile::tempdir().unwrap();
        let scratch = cache.path().join("stack-scratch");
        let left = scratch.join(".seiza-reintegrate-4242-Ab3xYz");
        let running = scratch.join(".seiza-reintegrate-4343-Cd4zWx");
        for directory in [&left, &running] {
            fs::create_dir_all(directory).unwrap();
            fs::write(directory.join("frame-0.f32"), b"samples").unwrap();
        }
        age(&left, 25 * 60 * 60);
        age(&running, 60);

        prune(cache.path(), &keep(&[]), "test");

        assert!(!left.exists(), "a crashed pass's scratch must be swept");
        assert!(running.exists(), "a running pass's scratch must stay");
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

    /// A mono or WBPP index entry for one target and filter.
    fn mono(job: &str, index: u32, target: i32, filter: &str, built: i64) -> serde_json::Value {
        serde_json::json!({"job_id": job, "created_unix_seconds": built,
            "group": {"index": index, "target_id": target, "target_name": format!("Target {target}"),
                "filter_name": filter}})
    }

    fn write_index(path: &Path, project: i32, list: &str, entries: Vec<serde_json::Value>) {
        fs::write(
            path,
            serde_json::to_vec(
                &serde_json::json!({"project_id": project, "database_id": "db", list: entries}),
            )
            .unwrap(),
        )
        .unwrap();
    }

    fn jobs(path: &Path, list: &str) -> Vec<String> {
        let index: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        index[list]
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| {
                format!(
                    "{}/{}",
                    entry["job_id"].as_str().unwrap(),
                    entry["group"]["filter_name"].as_str().unwrap_or("-")
                )
            })
            .collect()
    }

    #[test]
    fn a_moved_targets_entries_move_to_its_new_projects_indices() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(stacks.join("color")).unwrap();
        // Job "a" built targets 7 and 8; job "b" built target 7's Ha.
        write_index(
            &stacks.join("latest-project-2.json"),
            2,
            "groups",
            vec![
                mono("a", 0, 7, "L", 100),
                mono("a", 1, 8, "L", 100),
                mono("b", 0, 7, "Ha", 200),
            ],
        );
        // Project 5 has an older build of target 7's L, and another target.
        write_index(
            &stacks.join("latest-project-5.json"),
            5,
            "groups",
            vec![mono("old", 0, 7, "L", 50), mono("c", 0, 9, "L", 300)],
        );
        write_index(
            &stacks.join("wbpp-project-2.json"),
            2,
            "groups",
            vec![mono("w", 0, 7, "L", 100)],
        );
        write_index(
            &stacks.join("color/latest-project-2.json"),
            2,
            "jobs",
            vec![
                serde_json::json!({"job_id": "rgb", "project_id": 2, "target_id": 7, "kind": "rgb"}),
                serde_json::json!({"job_id": "other", "project_id": 2, "target_id": 8, "kind": "rgb"}),
            ],
        );

        let moved = move_target_indices(cache.path(), 7, 2, 5);

        assert_eq!(moved, 4);
        assert_eq!(
            jobs(&stacks.join("latest-project-2.json"), "groups"),
            ["a/L"]
        );
        assert_eq!(
            jobs(&stacks.join("latest-project-5.json"), "groups"),
            ["a/L", "c/L", "b/Ha"],
            "the later build of target 7's L replaces the older one"
        );
        let wbpp: serde_json::Value =
            serde_json::from_slice(&fs::read(stacks.join("wbpp-project-5.json")).unwrap()).unwrap();
        assert_eq!(
            (wbpp["project_id"].clone(), wbpp["database_id"].clone()),
            (5.into(), "db".into())
        );
        assert_eq!(jobs(&stacks.join("wbpp-project-5.json"), "groups"), ["w/L"]);
        assert!(jobs(&stacks.join("wbpp-project-2.json"), "groups").is_empty());
        let color: serde_json::Value =
            serde_json::from_slice(&fs::read(stacks.join("color/latest-project-5.json")).unwrap())
                .unwrap();
        assert_eq!(color["project_id"], 5);
        assert_eq!(color["jobs"][0]["job_id"], "rgb");
        assert_eq!(color["jobs"][0]["project_id"], 5);
        assert_eq!(
            jobs(&stacks.join("color/latest-project-2.json"), "jobs"),
            ["other/-"]
        );

        // An unreadable destination leaves the source as it was.
        fs::write(stacks.join("latest-project-6.json"), "{not json").unwrap();
        assert_eq!(move_target_indices(cache.path(), 8, 2, 6), 1);
        assert_eq!(
            jobs(&stacks.join("latest-project-2.json"), "groups"),
            ["a/L"]
        );
        assert_eq!(
            jobs(&stacks.join("color/latest-project-6.json"), "jobs"),
            ["other/-"]
        );
    }

    #[test]
    fn a_target_moved_away_and_back_keeps_its_latest_builds() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(&stacks).unwrap();
        write_index(
            &stacks.join("latest-project-1.json"),
            1,
            "groups",
            vec![mono("first", 0, 7, "L", 100)],
        );
        move_target_indices(cache.path(), 7, 1, 2);
        // A build that started before the move finishes into project 1.
        write_index(
            &stacks.join("latest-project-1.json"),
            1,
            "groups",
            vec![mono("late", 0, 7, "L", 200)],
        );
        // In project 2, a new Ha build.
        write_index(
            &stacks.join("latest-project-2.json"),
            2,
            "groups",
            vec![mono("first", 0, 7, "L", 100), mono("ha", 0, 7, "Ha", 150)],
        );
        assert_eq!(move_target_indices(cache.path(), 7, 2, 1), 2);
        assert_eq!(
            jobs(&stacks.join("latest-project-1.json"), "groups"),
            ["late/L", "ha/Ha"],
            "the later L build stays; the older one coming back does not replace it"
        );
        assert!(jobs(&stacks.join("latest-project-2.json"), "groups").is_empty());
    }

    #[test]
    fn the_sweep_moves_entries_a_build_wrote_after_its_target_moved() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(stacks.join("color")).unwrap();
        // Target 7 is in project 2 now; a build wrote it to project 1's index.
        write_index(
            &stacks.join("latest-project-1.json"),
            1,
            "groups",
            vec![
                mono("late", 0, 7, "L", 200),
                mono("stays", 0, 8, "L", 200),
                mono("gone", 0, 99, "L", 1),
            ],
        );
        write_index(
            &stacks.join("color/latest-project-1.json"),
            1,
            "jobs",
            vec![
                serde_json::json!({"job_id": "rgb", "project_id": 1, "target_id": 7,
                "target_name": " target 7 ", "kind": "rgb"}),
            ],
        );
        let targets = std::collections::HashMap::from([
            (7, (2, "Target 7".to_string())),
            (8, (1, "Target 8".to_string())),
        ]);

        assert_eq!(rehome_moved_target_entries(cache.path(), &targets), 2);

        assert_eq!(
            jobs(&stacks.join("latest-project-1.json"), "groups"),
            ["stays/L", "gone/L"],
            "an entry of a target the catalog no longer has stays"
        );
        assert_eq!(
            jobs(&stacks.join("latest-project-2.json"), "groups"),
            ["late/L"]
        );
        assert_eq!(
            jobs(&stacks.join("color/latest-project-2.json"), "jobs"),
            ["rgb/-"]
        );
        assert_eq!(rehome_moved_target_entries(cache.path(), &targets), 0);
    }

    #[test]
    fn the_sweep_leaves_a_deleted_targets_stacks_when_a_new_target_reuses_its_id() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(&stacks).unwrap();
        // Target 50 ("Target 50") was deleted; a new target in project 2
        // got its ID.
        write_index(
            &stacks.join("latest-project-1.json"),
            1,
            "groups",
            vec![mono("old", 0, 50, "L", 100)],
        );
        let targets = std::collections::HashMap::from([(50, (2, "NGC 891".to_string()))]);
        assert_eq!(rehome_moved_target_entries(cache.path(), &targets), 0);
        assert_eq!(
            jobs(&stacks.join("latest-project-1.json"), "groups"),
            ["old/L"]
        );
        assert!(!stacks.join("latest-project-2.json").exists());
    }

    #[test]
    fn a_merged_away_targets_entries_are_removed() {
        let cache = tempfile::tempdir().unwrap();
        let stacks = cache.path().join("stack-previews");
        fs::create_dir_all(stacks.join("color")).unwrap();
        write_index(
            &stacks.join("latest-project-1.json"),
            1,
            "groups",
            vec![mono("merged", 0, 7, "L", 100), mono("kept", 0, 8, "L", 100)],
        );
        write_index(
            &stacks.join("color/latest-project-1.json"),
            1,
            "jobs",
            vec![
                serde_json::json!({"job_id": "rgb", "project_id": 1, "target_id": 7, "kind": "rgb"}),
            ],
        );
        assert_eq!(drop_target_entries(cache.path(), 1, 7), 2);
        assert_eq!(
            jobs(&stacks.join("latest-project-1.json"), "groups"),
            ["kept/L"]
        );
        assert!(jobs(&stacks.join("color/latest-project-1.json"), "jobs").is_empty());
        assert_eq!(drop_target_entries(cache.path(), 1, 7), 0);
    }
}
