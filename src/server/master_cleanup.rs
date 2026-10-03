//! Removing calibration masters nothing needs.
//!
//! A master's file name hashes its inputs and the build's version, so a new
//! version, a new upstream master or a changed masking mode writes a new file
//! from the same frames and the old one is never read again. Each hour
//! [`sweep`] removes from a database's master folder:
//!
//! - files a stopped build left half written (`*.fits.tmp-*`), a day old;
//! - masters a newer master built from exactly the same frames replaced,
//!   once unused for a month and named by no stack.
//!
//! Over its volume's limit the disk limit may also take masters unused for a
//! week, least recently used first, those a stack names last
//! ([`lru_candidates`]), but only when that can bring the volume back under.
//!
//! Never removed:
//! - a master the catalog does not record, or whose source frames are gone:
//!   neither could be built again;
//! - anything when the catalog is read-only or newer than this build, since
//!   a removed master could not be recorded again;
//! - anything when a stack index or manifest cannot be read, since it might
//!   name the master;
//! - a master's catalog record, which holds its provenance;
//! - anything while a build of this server runs: the deletes take the build
//!   permit.
//!
//! A removed master that is needed again is built again from its frames.

use crate::calibration::{self, RecordedMasterFile};
use crate::server::database_context::DatabaseContext;
use crate::server::state::AppState;
use crate::server::storage;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Age of a half-written file before it goes.
const DEBRIS_AGE: Duration = Duration::from_secs(24 * 60 * 60);
/// How long a replaced master stays unused before it goes.
const REPLACED_UNUSED: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// How long a master stays unused before the disk limit may take it.
const LRU_UNUSED: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// A master file the disk limit may remove, with when it was last used.
#[derive(Debug, Clone)]
pub struct MasterCandidate {
    pub path: PathBuf,
    pub bytes: u64,
    pub last_used: SystemTime,
    /// A stack names it; such masters go after every other.
    pub referenced: bool,
}

/// Remove half-written files and replaced masters from one database's
/// master folder. Returns how many files went.
pub fn sweep(state: &AppState, ctx: &DatabaseContext, now: SystemTime) -> usize {
    let folder = storage::calibration_masters(&ctx.calibration_root);
    let debris: Vec<PathBuf> = std::fs::read_dir(&folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains(".fits.tmp-"))
                && age(path, now, Modified).is_some_and(|age| age >= DEBRIS_AGE)
        })
        .collect();
    let replaced = replaced_masters(ctx, &folder, now);
    if debris.is_empty() && replaced.is_empty() {
        return 0;
    }
    // A build reads masters and writes staged files: none may run meanwhile.
    let Some(_no_build) = state.stack_previews.try_maintenance_permit() else {
        return 0;
    };
    // A build may have reused one since it was chosen.
    let still_unused =
        |path: &&PathBuf| age(path, now, Used).is_some_and(|unused| unused >= REPLACED_UNUSED);
    let removed = debris
        .iter()
        .chain(replaced.iter().filter(still_unused))
        .filter(|path| remove(path))
        .count();
    if removed > 0 {
        tracing::info!(
            db = %ctx.id,
            removed,
            "Removed half-written and replaced calibration masters"
        );
    }
    removed
}

/// Masters in `folder` that a newer master from the same frames replaced,
/// unused for a month and named by no stack.
fn replaced_masters(ctx: &DatabaseContext, folder: &Path, now: SystemTime) -> Vec<PathBuf> {
    let Some(masters) = masters_in_folder(ctx, folder) else {
        return Vec::new();
    };
    let Some(referenced) = referenced_labels(&ctx.stack_root) else {
        return Vec::new();
    };
    let mut newest: HashMap<&str, i64> = HashMap::new();
    for (_, record) in &masters {
        let entry = newest
            .entry(record.sources.as_str())
            .or_insert(record.created_at);
        *entry = (*entry).max(record.created_at);
    }
    masters
        .iter()
        .filter(|(path, record)| {
            newest
                .get(record.sources.as_str())
                .is_some_and(|newest| *newest > record.created_at)
                && record.rebuildable
                && !label(path).is_some_and(|label| referenced.contains(label))
                && age(path, now, Used).is_some_and(|unused| unused >= REPLACED_UNUSED)
        })
        .map(|(path, _)| path.clone())
        .collect()
}

/// Masters of one database the disk limit may remove, least recently used
/// first, those no stack names first of all. Only files in its master
/// folder, recorded and rebuildable.
pub fn lru_candidates(ctx: &DatabaseContext, now: SystemTime) -> Vec<MasterCandidate> {
    let folder = storage::calibration_masters(&ctx.calibration_root);
    let Some(masters) = masters_in_folder(ctx, &folder) else {
        return Vec::new();
    };
    let Some(referenced) = referenced_labels(&ctx.stack_root) else {
        return Vec::new();
    };
    let mut candidates: Vec<MasterCandidate> = masters
        .into_iter()
        .filter(|(_, record)| record.rebuildable)
        .filter_map(|(path, _)| {
            let metadata = std::fs::metadata(&path).ok()?;
            let last_used = last_used(&metadata);
            let unused = now.duration_since(last_used).ok()?;
            (unused >= LRU_UNUSED).then(|| MasterCandidate {
                referenced: label(&path).is_some_and(|label| referenced.contains(label)),
                path,
                bytes: metadata.len(),
                last_used,
            })
        })
        .collect();
    candidates.sort_by_key(|candidate| (candidate.referenced, candidate.last_used));
    candidates
}

/// The master files in `folder` the catalog records, matched by file name
/// (a content hash, so a moved folder still matches), with their records.
/// `None` when nothing may be removed: the catalog cannot record a master
/// again, cannot be read, or records none of the files here.
fn masters_in_folder(
    ctx: &DatabaseContext,
    folder: &Path,
) -> Option<Vec<(PathBuf, RecordedMasterFile)>> {
    {
        // The shared connection says whether a removed master could be
        // recorded again; it is only borrowed when free.
        let connection = ctx.db();
        let connection = connection.try_lock().ok()?;
        if !calibration::master_catalog_rebuilds(&connection) {
            return None;
        }
    }
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        &ctx.database_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let records = match calibration::recorded_master_files(&connection) {
        Ok(records) => records,
        Err(error) => {
            tracing::warn!(db = %ctx.id, "Not cleaning calibration masters: {error}");
            return None;
        }
    };
    let by_name: HashMap<String, RecordedMasterFile> = records
        .into_iter()
        .filter_map(|record| Some((label(&record.cache_path)?.to_string(), record)))
        .collect();
    let masters: Vec<(PathBuf, RecordedMasterFile)> = std::fs::read_dir(folder)
        .ok()?
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter_map(|entry| {
            let name = entry.file_name().to_str()?.to_string();
            let record = by_name.get(&name)?.clone();
            Some((entry.path(), record))
        })
        .collect();
    (!masters.is_empty()).then_some(masters)
}

/// Every master file name a stack of this database names: in its indices
/// and in every job's manifest, read as text, so every place one is
/// recorded counts. `None` when any of them cannot be read.
pub fn referenced_labels(stack_root: &Path) -> Option<HashSet<String>> {
    let read_folder = |folder: &Path| -> Option<Vec<PathBuf>> {
        match std::fs::read_dir(folder) {
            Ok(entries) => Some(entries.flatten().map(|entry| entry.path()).collect()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(Vec::new()),
            Err(_) => None,
        }
    };
    let mut files = Vec::new();
    for folder in [
        storage::stacks(stack_root),
        storage::stack_folder(stack_root, storage::stack_kind::COLOR),
    ] {
        let Some(entries) = read_folder(&folder) else {
            tracing::warn!(
                "Not removing calibration masters: {} could not be listed",
                folder.display()
            );
            return None;
        };
        for path in entries {
            if path.is_dir() {
                let manifest = path.join("manifest.json");
                if manifest.is_file() {
                    files.push(manifest);
                }
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                files.push(path);
            }
        }
    }
    let mut labels = HashSet::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            tracing::warn!(
                "Not removing calibration masters: {} could not be read, and it may name one",
                file.display()
            );
            return None;
        };
        labels.extend(labels_in(&text));
    }
    Some(labels)
}

/// Master labels (`<kind>-<64 hex>.fits`) in a text.
fn labels_in(text: &str) -> impl Iterator<Item = String> + '_ {
    text.match_indices(".fits").filter_map(|(end, _)| {
        let start = text[..end]
            .rfind(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .map_or(0, |index| index + 1);
        let candidate = &text[start..end + ".fits".len()];
        is_master_label(candidate).then(|| candidate.to_string())
    })
}

fn is_master_label(name: &str) -> bool {
    ["bias", "dark", "dark_flat", "flat"].iter().any(|kind| {
        name.strip_prefix(kind)
            .and_then(|rest| rest.strip_prefix('-'))
            .and_then(|rest| rest.strip_suffix(".fits"))
            .is_some_and(|hash| hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
    })
}

fn label(path: &Path) -> Option<&str> {
    path.file_name().and_then(|name| name.to_str())
}

/// When a master was last used: its access time, which reuse sets
/// explicitly, else its modification time.
fn last_used(metadata: &std::fs::Metadata) -> SystemTime {
    metadata
        .accessed()
        .ok()
        .max(metadata.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH)
}

#[derive(Clone, Copy)]
enum Since {
    Modified,
    Used,
}
use Since::{Modified, Used};

fn age(path: &Path, now: SystemTime, since: Since) -> Option<Duration> {
    let metadata = std::fs::metadata(path).ok()?;
    let at = match since {
        Modified => metadata.modified().ok()?,
        Used => last_used(&metadata),
    };
    now.duration_since(at).ok()
}

fn remove(path: &Path) -> bool {
    match std::fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(
                "could not remove calibration master {}: {error}",
                path.display()
            );
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_found_wherever_an_index_names_them() {
        let hash = "a".repeat(64);
        let text = format!(
            r#"{{"flat_master":"flat-{hash}.fits","masters_signature":"bias=bias-{hash}.fits;dark=none","x":"notes.fits"}}"#
        );
        let found: HashSet<String> = labels_in(&text).collect();
        assert_eq!(
            found,
            [format!("flat-{hash}.fits"), format!("bias-{hash}.fits")]
                .into_iter()
                .collect()
        );
        assert!(is_master_label(&format!("dark_flat-{hash}.fits")));
        assert!(!is_master_label("flat-short.fits"));
    }

    fn aged(path: &Path, days: u64) {
        let then = SystemTime::now() - Duration::from_secs(days * 86_400);
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

    struct Fixture {
        _temp: tempfile::TempDir,
        state: AppState,
        ctx: DatabaseContext,
        folder: PathBuf,
    }

    fn label_of(fill: char) -> String {
        format!("dark-{}.fits", fill.to_string().repeat(64))
    }

    /// A catalog with one present frame, and masters `(fill, created,
    /// sources)` recorded and written, all unused for two months.
    fn fixture(masters: &[(char, i64, &str)]) -> Fixture {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("catalog.sqlite");
        crate::ts_schema::create_fresh_db(&database).unwrap();
        let images = temp.path().join("images");
        std::fs::create_dir_all(&images).unwrap();
        let ctx = DatabaseContext::new(
            "db".into(),
            "Db".into(),
            database.to_string_lossy().into_owned(),
            vec![images.to_string_lossy().into_owned()],
            None,
            None,
            None,
            temp.path().join("cache"),
        )
        .unwrap();
        let folder = storage::calibration_masters(&ctx.calibration_root);
        std::fs::create_dir_all(&folder).unwrap();
        let frame = images.join("dark-001.fits");
        std::fs::write(&frame, "frame").unwrap();
        {
            let connection = ctx.db();
            let connection = connection.lock().unwrap();
            calibration::ensure_schema(&connection).unwrap();
            connection
                .execute(
                    "INSERT INTO psf_guard_calibration_frame
                        (frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                         added_at, updated_at)
                     VALUES ('present', 'rig', 'dark', ?1, 'f', 0, 0)",
                    [frame.to_string_lossy()],
                )
                .unwrap();
            for (fill, created, sources) in masters {
                connection
                    .execute(
                        "INSERT INTO psf_guard_calibration_master
                            (master_uuid, rig_uuid, kind, cache_path, source_set_hash,
                             source_count, source_frame_uuids, created_at, seiza_version,
                             cache_version, statistics_json)
                         VALUES (?1, 'rig', 'dark', ?2, 'hash', 1, ?3, ?4, 'v', 1, '{}')",
                        rusqlite::params![
                            fill.to_string(),
                            folder.join(label_of(*fill)).to_string_lossy(),
                            sources,
                            created
                        ],
                    )
                    .unwrap();
                let path = folder.join(label_of(*fill));
                std::fs::write(&path, "master").unwrap();
                aged(&path, 60);
            }
        }
        let state = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        Fixture {
            _temp: temp,
            state,
            ctx,
            folder,
        }
    }

    #[test]
    fn the_sweep_removes_debris_and_masters_replaced_from_the_same_frames() {
        let present = r#"["present"]"#;
        let f = fixture(&[
            ('a', 1, present), // replaced by e
            ('b', 2, present), // replaced, but a stack's manifest names it
            ('c', 3, r#"["gone"]"#),
            ('d', 4, present), // replaced, but used this week
            ('e', 5, present), // the newest from these frames
        ]);
        aged(&f.folder.join(label_of('d')), 3);
        let stacks = storage::stacks(&f.ctx.stack_root);
        std::fs::create_dir_all(stacks.join("job")).unwrap();
        // An old job a color stack may still use names b.
        std::fs::write(
            stacks.join("job").join("manifest.json"),
            format!(r#"{{"dark_master":"{}"}}"#, label_of('b')),
        )
        .unwrap();
        let tmp = f.folder.join(format!("{}.tmp-1-2", label_of('f')));
        let unrecorded = f.folder.join(label_of('9'));
        for path in [&tmp, &unrecorded] {
            std::fs::write(path, "debris").unwrap();
            aged(path, 2);
        }

        let removed = sweep(&f.state, &f.ctx, SystemTime::now());

        assert_eq!(removed, 2);
        assert!(!tmp.exists());
        assert!(!f.folder.join(label_of('a')).exists());
        assert!(
            unrecorded.exists(),
            "unrecorded: nothing shows it can be rebuilt"
        );
        for kept in ['b', 'c', 'd', 'e'] {
            assert!(f.folder.join(label_of(kept)).exists(), "{kept}");
        }
    }

    #[test]
    fn masters_from_other_frames_are_not_replacements() {
        // Same kind, same rig, built from other frames: other lights (another
        // night's flats, another gain's darks) may need each.
        let f = fixture(&[
            ('a', 1, r#"["present"]"#),
            ('b', 2, r#"["present","other"]"#),
        ]);

        assert_eq!(sweep(&f.state, &f.ctx, SystemTime::now()), 0);
        assert!(f.folder.join(label_of('a')).exists());
    }

    #[test]
    fn an_unreadable_stack_index_removes_nothing() {
        let f = fixture(&[('a', 1, r#"["present"]"#), ('b', 2, r#"["present"]"#)]);
        let stacks = storage::stacks(&f.ctx.stack_root);
        std::fs::create_dir_all(&stacks).unwrap();
        // Not text: it cannot be told whether it names a.
        std::fs::write(stacks.join("latest-project-1.json"), [0xff, 0xfe]).unwrap();

        assert_eq!(sweep(&f.state, &f.ctx, SystemTime::now()), 0);
        assert!(lru_candidates(&f.ctx, SystemTime::now()).is_empty());
        assert!(f.folder.join(label_of('a')).exists());
    }

    #[test]
    fn the_disk_limit_takes_only_rebuildable_masters() {
        let f = fixture(&[
            ('a', 1, r#"["present"]"#),
            ('b', 2, r#"["present","x"]"#),
            ('c', 3, r#"["gone"]"#),
        ]);

        let paths: Vec<String> = lru_candidates(&f.ctx, SystemTime::now())
            .into_iter()
            .map(|candidate| label(&candidate.path).unwrap().to_string())
            .collect();

        // b and c each name a frame the catalog does not have on disk.
        assert_eq!(paths, [label_of('a')]);
    }
}
