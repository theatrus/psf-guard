//! Moving generated files when a storage folder changes in Settings.
//!
//! Saving a folder in Settings records a move from the folder in use to the
//! new one (`storage.moves` in the registry). The next process start carries
//! it out before any database opens; a folder named on the command line or
//! in the config file never moves anything, as before.
//!
//! Each file goes by rename on the same file system, else by a copy into a
//! `.partial` sibling that is renamed into place before the source is
//! removed. Folders merge: a file already at the target with the same size
//! and modification time is taken as copied, so a start that stopped halfway
//! finishes the move on the next one. A file that differs is a conflict. A
//! kind whose move fails is moved back and stays in its old folder, with a
//! note saying why, and its move is tried again at the next start.
//!
//! Folders that belong to another kind are never carried along, even when
//! they sit inside the folder being moved.

use super::{StorageKind, StorageRoots};
use std::path::{Path, PathBuf};

const PARTIAL_SUFFIX: &str = ".partial";

/// One folder to move, as recorded when Settings saved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Move {
    pub kind: StorageKind,
    pub from: PathBuf,
    pub to: PathBuf,
}

/// What one start's moves did.
#[derive(Debug, Default)]
pub struct Relocated {
    /// Kinds whose move finished.
    pub finished: Vec<StorageKind>,
    /// One line per folder moved or kept back, for the log and Settings.
    pub notes: Vec<String>,
    /// Calibration master folders that moved, per database slug, so their
    /// catalog rows can follow.
    pub calibration_moves: Vec<(String, PathBuf, PathBuf)>,
}

/// Carry out `moves`, stacks and calibration masters before the cache, so
/// the cache move takes only what is left. `roots` holds the folders to run
/// with, the moves' targets included; a kind whose move fails is set back to
/// its old folder. Nothing below any folder in `roots` or in a move is
/// carried along by another kind's move.
pub fn relocate(moves: &[Move], roots: &mut StorageRoots) -> Relocated {
    let mut result = Relocated::default();
    let mut protected: Vec<PathBuf> = StorageKind::ALL
        .iter()
        .map(|kind| normalized(roots.get(*kind)))
        .chain(
            moves
                .iter()
                .flat_map(|step| [normalized(&step.from), normalized(&step.to)]),
        )
        .collect();
    protected.sort();
    protected.dedup();

    for kind in [
        StorageKind::Stacks,
        StorageKind::Calibration,
        StorageKind::Cache,
    ] {
        let Some(step) = moves.iter().find(|step| step.kind == kind) else {
            continue;
        };
        if same_folder(&step.from, &step.to) {
            result.finished.push(kind);
            continue;
        }
        let mut done: Vec<(PathBuf, PathBuf)> = Vec::new();
        let mut calibration = Vec::new();
        let outcome = match kind {
            StorageKind::Cache => {
                // Folders the other kinds own stay when those kinds now live
                // in the old cache folder.
                let staying: Vec<&str> = [StorageKind::Stacks, StorageKind::Calibration]
                    .into_iter()
                    .filter(|other| same_folder(roots.get(*other), &step.from))
                    .flat_map(|other| other.database_folders().iter().copied())
                    .collect();
                move_cache(&step.from, &step.to, &protected, &staying, &mut done)
            }
            _ => move_database_folders(kind, step, &protected, &mut done, &mut calibration),
        };
        match outcome {
            Ok(()) => {
                result.finished.push(kind);
                result.calibration_moves.extend(calibration);
                if !done.is_empty() {
                    result.notes.push(format!(
                        "Moved the {} from {} to {}.",
                        kind.label(),
                        step.from.display(),
                        step.to.display()
                    ));
                }
            }
            Err(error) => {
                let mut note = format!(
                    "Kept the {} in {}: moving them to {} failed: {error}. The move is tried \
                     again at the next start.",
                    kind.label(),
                    step.from.display(),
                    step.to.display()
                );
                // Put back what did move, so the old folder is whole again.
                for (source, target) in done.iter().rev() {
                    if let Err(back) = merge_move(target, source) {
                        note.push_str(&format!(
                            " Moving {} back also failed ({back}); its files are in {}.",
                            source.display(),
                            target.display()
                        ));
                    }
                }
                result.notes.push(note);
                roots.set(kind, step.from.clone());
            }
        }
    }
    result
}

/// What renaming a database's slug folder did in every storage folder.
#[derive(Debug, Default)]
pub struct Renamed {
    pub failures: Vec<String>,
    /// Whether the calibration folder's slug folder moved, so the master
    /// rows should follow.
    pub calibration_moved: bool,
}

/// Rename one database's slug folder in every storage folder, when the
/// database gets a new slug. Failures are returned, not raised: the worst
/// case is that the files are generated again under the new slug.
pub fn rename_database(roots: &StorageRoots, old_id: &str, new_id: &str) -> Renamed {
    let mut renamed = Renamed::default();
    let mut seen: Vec<PathBuf> = Vec::new();
    for kind in StorageKind::ALL {
        let root = roots.get(kind);
        let first_time = !seen.iter().any(|done| same_folder(done, root));
        if first_time {
            seen.push(root.to_path_buf());
        }
        let moved = if first_time {
            match merge_move(&root.join(old_id), &root.join(new_id)) {
                Ok(_) => true,
                Err(error) => {
                    renamed.failures.push(format!(
                        "could not move {} to {}: {error}",
                        root.join(old_id).display(),
                        root.join(new_id).display()
                    ));
                    false
                }
            }
        } else {
            // Same folder as an earlier kind: it moved, or failed, then.
            renamed.failures.is_empty()
        };
        if kind == StorageKind::Calibration {
            renamed.calibration_moved = moved;
        }
    }
    renamed
}

/// Point a database's calibration master rows at the folder its masters
/// moved to. Logged, never raised: a row left behind only means that master
/// is built again.
pub fn follow_master_rows(
    ctx: &crate::server::database_context::DatabaseContext,
    from: &Path,
    to: &Path,
) {
    let connection = ctx.db();
    let Ok(mut connection) = connection.lock() else {
        return;
    };
    match crate::calibration::rewrite_master_paths(&mut connection, from, to) {
        Ok(0) => {}
        Ok(rows) => tracing::info!(
            "📦 {}: {rows} calibration master records now point at {}",
            ctx.id,
            to.display()
        ),
        Err(error) => tracing::warn!(
            "📦 {}: calibration master records still name {}: {error}. Those masters \
             will be built again.",
            ctx.id,
            from.display()
        ),
    }
}

/// Whether two paths name the same folder, made absolute against the
/// working directory and with `.` and `..` resolved, without touching the
/// disk: either may not exist yet.
pub fn same_folder(left: &Path, right: &Path) -> bool {
    normalized(left) == normalized(right)
}

/// Whether one folder is inside the other, or they are the same.
pub fn overlaps(left: &Path, right: &Path) -> bool {
    let (left, right) = (normalized(left), normalized(right));
    left.starts_with(&right) || right.starts_with(&left)
}

pub fn normalized(path: &Path) -> PathBuf {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut clean = PathBuf::new();
    for component in absolute.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    clean
}

/// Whether `path` is one of the protected folders or holds one.
fn holds_protected(path: &Path, protected: &[PathBuf]) -> bool {
    let path = normalized(path);
    protected.iter().any(|folder| folder.starts_with(&path))
}

/// Move one kind's named folders out of every database folder in the old
/// folder.
fn move_database_folders(
    kind: StorageKind,
    step: &Move,
    protected: &[PathBuf],
    done: &mut Vec<(PathBuf, PathBuf)>,
    calibration: &mut Vec<(String, PathBuf, PathBuf)>,
) -> Result<(), String> {
    for database in database_folders(&step.from)? {
        let Some(slug) = database.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // A storage folder kept inside the old one is not a database.
        if holds_protected(&database, protected) {
            continue;
        }
        let mut moved_any = false;
        for name in kind.database_folders() {
            let source = database.join(name);
            let target = step.to.join(slug).join(name);
            if merge_move(&source, &target)? {
                done.push((source, target));
                moved_any = true;
            }
        }
        if moved_any && kind == StorageKind::Calibration {
            calibration.push((slug.to_string(), database.clone(), step.to.join(slug)));
        }
    }
    Ok(())
}

/// Move everything in the old cache folder except other kinds' folders:
/// those staying in it, and any storage folder kept inside it.
fn move_cache(
    from: &Path,
    to: &Path,
    protected: &[PathBuf],
    staying: &[&str],
    done: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    if overlaps(from, to) {
        return Err("one folder is inside the other".into());
    }
    let protected: Vec<PathBuf> = protected
        .iter()
        .filter(|folder| **folder != normalized(from))
        .cloned()
        .collect();
    move_cache_entries(from, to, &protected, staying, 0, done)
}

fn move_cache_entries(
    from: &Path,
    to: &Path,
    protected: &[PathBuf],
    staying: &[&str],
    depth: usize,
    done: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    let entries = match std::fs::read_dir(from) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("reading {}: {error}", from.display())),
    };
    for entry in entries.flatten() {
        let source = entry.path();
        let target = to.join(entry.file_name());
        // `<cache>/<slug>/<name>` for a kind that stays in the old folder.
        if depth == 1 && staying.iter().any(|name| entry.file_name() == *name) {
            continue;
        }
        let normalized_source = normalized(&source);
        if protected.contains(&normalized_source) {
            continue;
        }
        let holds_other = protected
            .iter()
            .any(|folder| folder.starts_with(&normalized_source));
        let holds_staying =
            depth == 0 && source.is_dir() && staying.iter().any(|name| source.join(name).exists());
        if holds_other || holds_staying {
            move_cache_entries(&source, &target, protected, staying, depth + 1, done)?;
        } else if merge_move(&source, &target)? {
            done.push((source, target));
        }
    }
    Ok(())
}

/// Folders directly in `root`; a missing root holds none.
fn database_folders(root: &Path) -> Result<Vec<PathBuf>, String> {
    match std::fs::read_dir(root) {
        Ok(entries) => Ok(entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.is_dir())
            .collect()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(format!("reading {}: {error}", root.display())),
    }
}

/// Move a file or folder into place, merging into a folder already there.
/// A file already at the target with the same size and modification time
/// counts as moved; one that differs is a conflict. Returns whether there
/// was anything to move.
pub fn merge_move(source: &Path, target: &Path) -> Result<bool, String> {
    let Ok(source_metadata) = std::fs::symlink_metadata(source) else {
        return Ok(false);
    };
    let Ok(target_metadata) = std::fs::symlink_metadata(target) else {
        move_whole(source, target)?;
        return Ok(true);
    };
    if source_metadata.is_dir() && target_metadata.is_dir() {
        let children = std::fs::read_dir(source)
            .map_err(|error| format!("reading {}: {error}", source.display()))?;
        for child in children.flatten() {
            merge_move(&child.path(), &target.join(child.file_name()))?;
        }
        std::fs::remove_dir(source)
            .map_err(|error| format!("removing {}: {error}", source.display()))?;
        return Ok(true);
    }
    let same_file = source_metadata.is_file()
        && target_metadata.is_file()
        && source_metadata.len() == target_metadata.len()
        && source_metadata.modified().ok() == target_metadata.modified().ok();
    if same_file {
        std::fs::remove_file(source)
            .map_err(|error| format!("removing {}: {error}", source.display()))?;
        return Ok(true);
    }
    Err(format!(
        "{} already exists and differs from {}",
        target.display(),
        source.display()
    ))
}

/// Move a file or folder to a target that does not exist.
fn move_whole(source: &Path, target: &Path) -> Result<(), String> {
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("creating {}: {error}", parent.display()))?;
    }
    match std::fs::rename(source, target) {
        Ok(()) => return Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::CrossesDevices => {}
        Err(error) => {
            return Err(format!(
                "moving {} to {}: {error}",
                source.display(),
                target.display()
            ));
        }
    }
    // Another file system: copy beside the target, then rename into place,
    // so the target never holds half a folder.
    let mut partial = target.as_os_str().to_owned();
    partial.push(PARTIAL_SUFFIX);
    let partial = PathBuf::from(partial);
    if std::fs::symlink_metadata(&partial).is_ok() {
        remove_any(&partial).map_err(|error| format!("clearing {}: {error}", partial.display()))?;
    }
    if let Err(error) = copy_tree(source, &partial) {
        let _ = remove_any(&partial);
        return Err(format!(
            "copying {} to {}: {error}",
            source.display(),
            partial.display()
        ));
    }
    std::fs::rename(&partial, target)
        .map_err(|error| format!("renaming {}: {error}", partial.display()))?;
    // The copy is whole; a source that will not go is finished next time,
    // when its files match the copies.
    remove_any(source).map_err(|error| {
        format!(
            "copied to {}, but removing {} failed: {error}",
            target.display(),
            source.display()
        )
    })
}

fn remove_any(path: &Path) -> std::io::Result<()> {
    if std::fs::symlink_metadata(path)?.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
}

/// Copy a file or folder, keeping file modification and access times: the
/// disk limit deletes least recently used files first and reads them, and a
/// resumed move compares them.
fn copy_tree(source: &Path, target: &Path) -> std::io::Result<()> {
    let metadata = std::fs::symlink_metadata(source)?;
    if metadata.is_dir() {
        std::fs::create_dir(target)?;
        for entry in std::fs::read_dir(source)? {
            let entry = entry?;
            copy_tree(&entry.path(), &target.join(entry.file_name()))?;
        }
        return Ok(());
    }
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        return std::os::unix::fs::symlink(std::fs::read_link(source)?, target);
        #[cfg(not(unix))]
        {
            std::fs::copy(source, target)?;
            return Ok(());
        }
    }
    std::fs::copy(source, target)?;
    let mut times = std::fs::FileTimes::new();
    if let Ok(accessed) = metadata.accessed() {
        times = times.set_accessed(accessed);
    }
    if let Ok(modified) = metadata.modified() {
        times = times.set_modified(modified);
    }
    // Write access: Windows needs it to set times.
    std::fs::OpenOptions::new()
        .write(true)
        .open(target)?
        .set_times(times)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn step(kind: StorageKind, from: &Path, to: &Path) -> Move {
        Move {
            kind,
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        }
    }

    #[test]
    fn stacks_and_masters_move_and_the_cache_stays() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        write(&cache.join("db/previews/1.png"), "preview");
        write(&cache.join("db/stack-previews/job/manifest.json"), "stack");
        write(&cache.join("db/stack-processing/a.json"), "selection");
        write(&cache.join("db/calibration-masters/dark-1.fits"), "master");
        let stacks = temp.path().join("stacks");
        let masters = temp.path().join("masters");
        let mut roots = StorageRoots {
            cache: cache.clone(),
            stacks: stacks.clone(),
            calibration: masters.clone(),
        };

        let moved = relocate(
            &[
                step(StorageKind::Stacks, &cache, &stacks),
                step(StorageKind::Calibration, &cache, &masters),
            ],
            &mut roots,
        );

        assert_eq!(roots.stacks, stacks);
        assert_eq!(moved.notes.len(), 2, "{:?}", moved.notes);
        assert!(stacks.join("db/stack-previews/job/manifest.json").is_file());
        assert!(stacks.join("db/stack-processing/a.json").is_file());
        assert!(masters.join("db/calibration-masters/dark-1.fits").is_file());
        assert!(cache.join("db/previews/1.png").is_file());
        assert!(!cache.join("db/stack-previews").exists());
        assert_eq!(
            moved.calibration_moves,
            vec![("db".to_string(), cache.join("db"), masters.join("db"))]
        );
    }

    #[test]
    fn moving_the_whole_cache_takes_stacks_and_masters_with_it() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        write(&old.join("db/previews/1.png"), "preview");
        write(&old.join("db/stack-previews/job/manifest.json"), "stack");
        write(&old.join("db/calibration-masters/dark-1.fits"), "master");
        write(&old.join("satellites/elements.json"), "elements");
        let mut roots = StorageRoots::single(&new);

        let moved = relocate(
            &[
                step(StorageKind::Stacks, &old, &new),
                step(StorageKind::Calibration, &old, &new),
                step(StorageKind::Cache, &old, &new),
            ],
            &mut roots,
        );

        assert_eq!(roots, StorageRoots::single(&new), "{:?}", moved.notes);
        assert_eq!(moved.finished.len(), 3);
        for file in [
            "db/previews/1.png",
            "db/stack-previews/job/manifest.json",
            "db/calibration-masters/dark-1.fits",
            "satellites/elements.json",
        ] {
            assert!(new.join(file).is_file(), "{file}");
        }
        assert!(!old.join("db").exists());
    }

    #[test]
    fn a_moved_cache_leaves_stacks_that_stay_behind() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        write(&old.join("db/previews/1.png"), "preview");
        write(&old.join("db/stack-previews/job/manifest.json"), "stack");
        let mut roots = StorageRoots {
            stacks: old.clone(),
            ..StorageRoots::single(&new)
        };

        relocate(&[step(StorageKind::Cache, &old, &new)], &mut roots);

        assert!(new.join("db/previews/1.png").is_file());
        assert!(old.join("db/stack-previews/job/manifest.json").is_file());
        assert!(!old.join("db/previews").exists());
    }

    #[test]
    fn a_stack_folder_inside_the_old_cache_is_not_carried_along() {
        let temp = tempfile::tempdir().unwrap();
        let old = temp.path().join("old");
        let new = temp.path().join("new");
        let stacks = old.join("stacks");
        write(&old.join("db/previews/1.png"), "preview");
        write(&stacks.join("db/stack-previews/job/manifest.json"), "stack");
        let mut roots = StorageRoots {
            stacks: stacks.clone(),
            ..StorageRoots::single(&new)
        };

        relocate(&[step(StorageKind::Cache, &old, &new)], &mut roots);

        assert!(new.join("db/previews/1.png").is_file());
        assert!(stacks.join("db/stack-previews/job/manifest.json").is_file());
        assert!(!new.join("stacks").exists());
    }

    #[test]
    fn a_failed_move_puts_back_what_moved_and_keeps_the_old_folder() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let stacks = temp.path().join("stacks");
        write(&cache.join("a/stack-previews/job/manifest.json"), "a");
        write(&cache.join("b/stack-previews/job/manifest.json"), "b");
        // Something different already sits where one database's stack goes.
        write(&stacks.join("a/stack-previews/job/manifest.json"), "other");
        write(&stacks.join("b/stack-previews/job/manifest.json"), "other");
        let mut roots = StorageRoots {
            stacks: stacks.clone(),
            ..StorageRoots::single(&cache)
        };

        let moved = relocate(&[step(StorageKind::Stacks, &cache, &stacks)], &mut roots);

        assert_eq!(roots.stacks, cache);
        assert!(moved.finished.is_empty());
        assert!(
            moved.notes[0].starts_with("Kept the stacks"),
            "{:?}",
            moved.notes
        );
        assert_eq!(
            std::fs::read_to_string(cache.join("a/stack-previews/job/manifest.json")).unwrap(),
            "a"
        );
        assert_eq!(
            std::fs::read_to_string(cache.join("b/stack-previews/job/manifest.json")).unwrap(),
            "b"
        );
    }

    #[test]
    fn an_interrupted_copy_finishes_on_the_next_start() {
        let temp = tempfile::tempdir().unwrap();
        let cache = temp.path().join("cache");
        let stacks = temp.path().join("stacks");
        write(&cache.join("db/stack-previews/job/a.fits"), "a");
        write(&cache.join("db/stack-previews/job/b.fits"), "b");
        // The copy landed but the source was not removed.
        copy_tree(&cache.join("db/stack-previews"), &{
            std::fs::create_dir_all(stacks.join("db")).unwrap();
            stacks.join("db/stack-previews")
        })
        .unwrap();
        let mut roots = StorageRoots {
            stacks: stacks.clone(),
            ..StorageRoots::single(&cache)
        };

        let moved = relocate(&[step(StorageKind::Stacks, &cache, &stacks)], &mut roots);

        assert_eq!(
            moved.finished,
            vec![StorageKind::Stacks],
            "{:?}",
            moved.notes
        );
        assert!(!cache.join("db/stack-previews").exists());
        assert!(stacks.join("db/stack-previews/job/b.fits").is_file());
    }

    #[test]
    fn a_copy_keeps_file_times() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        write(&source.join("a/b.txt"), "b");
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        std::fs::OpenOptions::new()
            .write(true)
            .open(source.join("a/b.txt"))
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(old)
                    .set_accessed(old),
            )
            .unwrap();

        copy_tree(&source, &temp.path().join("copy")).unwrap();

        let copied = std::fs::metadata(temp.path().join("copy/a/b.txt")).unwrap();
        assert_eq!(copied.modified().unwrap(), old);
    }

    #[test]
    fn renaming_a_database_moves_its_folder_in_each_root_once() {
        let temp = tempfile::tempdir().unwrap();
        let roots = StorageRoots {
            cache: temp.path().join("cache"),
            stacks: temp.path().join("stacks"),
            calibration: temp.path().join("cache"),
        };
        write(&roots.cache.join("old/previews/1.png"), "preview");
        write(&roots.stacks.join("old/stack-previews/a"), "stack");

        let renamed = rename_database(&roots, "old", "new");

        assert!(renamed.failures.is_empty(), "{:?}", renamed.failures);
        assert!(renamed.calibration_moved);
        assert!(roots.cache.join("new/previews/1.png").is_file());
        assert!(roots.stacks.join("new/stack-previews/a").is_file());
    }
}
