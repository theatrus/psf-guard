//! Removing calibration masters nothing needs.
//!
//! A master's file name hashes its inputs, so a changed frame set, a new
//! upstream master or a new build writes a new file and the old one stays.
//! Each hour, while no stack build runs, [`sweep`] removes from a
//! database's master folder:
//!
//! - files a crashed build left half written (`*.fits.tmp-*`), a day old;
//! - masters the catalog no longer records (forgotten frames, an older
//!   catalog), a day old;
//! - masters a newer master of the same rig, kind, exposure, filter and
//!   temperature replaced, once unused for a month and named by no stack.
//!
//! Under disk pressure the disk limit also takes masters least recently used
//! first ([`lru_candidates`]), unused for a week, those no stack names
//! before those one does.
//!
//! Never removed: a master whose source frames are gone (it cannot be built
//! again), a master used recently, a master's catalog record (it holds the
//! master's provenance and flat-stability judgement), and anything while a
//! build might be reading it. A removed master that is needed again is built
//! again from its frames.

use crate::calibration::{self, RecordedMasterFile};
use crate::server::database_context::DatabaseContext;
use crate::server::storage::{self, relocate::normalized};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Age of a half-written or unrecorded file before it goes.
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
    /// A kept stack names it; such masters go after every other.
    pub referenced: bool,
}

/// Remove debris and replaced masters from one database's master folder.
/// Returns how many files went. Reads the catalog on a connection of its
/// own; a catalog that cannot be read removes nothing.
pub fn sweep(ctx: &DatabaseContext, now: SystemTime) -> usize {
    let folder = storage::calibration_masters(&ctx.calibration_root);
    let Some(records) = recorded(ctx) else {
        return 0;
    };
    let referenced = referenced_labels(&ctx.stack_root);
    let recorded_paths: HashSet<PathBuf> = records
        .iter()
        .map(|record| normalized(&record.cache_path))
        .collect();
    let mut removed = 0;

    for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
        let path = entry.path();
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let debris = name.contains(".fits.tmp-")
            || (is_master_label(&name) && !recorded_paths.contains(&normalized(&path)));
        if debris && age(&path, now, Modified).is_some_and(|age| age >= DEBRIS_AGE) {
            removed += usize::from(remove(&path));
        }
    }

    // Within one identity, every master but the newest one on disk.
    let mut newest: HashMap<&str, i64> = HashMap::new();
    for record in records.iter().filter(|record| record.cache_path.is_file()) {
        let entry = newest
            .entry(record.identity.as_str())
            .or_insert(record.created_at);
        *entry = (*entry).max(record.created_at);
    }
    for record in &records {
        let path = &record.cache_path;
        let replaced = newest
            .get(record.identity.as_str())
            .is_some_and(|newest| *newest > record.created_at);
        if !replaced
            || !record.rebuildable
            || !path.is_file()
            || label(path).is_some_and(|label| referenced.contains(label))
            || age(path, now, Used).is_none_or(|unused| unused < REPLACED_UNUSED)
        {
            continue;
        }
        removed += usize::from(remove(path));
    }
    if removed > 0 {
        tracing::info!(
            db = %ctx.id,
            removed,
            "Removed calibration masters nothing needs"
        );
    }
    removed
}

/// Masters the disk limit may remove from one database, least recently
/// used first, those no stack names first of all.
pub fn lru_candidates(ctx: &DatabaseContext, now: SystemTime) -> Vec<MasterCandidate> {
    let Some(records) = recorded(ctx) else {
        return Vec::new();
    };
    let referenced = referenced_labels(&ctx.stack_root);
    let mut candidates: Vec<MasterCandidate> = records
        .iter()
        .filter(|record| record.rebuildable)
        .filter_map(|record| {
            let path = &record.cache_path;
            let metadata = std::fs::metadata(path)
                .ok()
                .filter(|metadata| metadata.is_file())?;
            let last_used = last_used(&metadata);
            let unused = now.duration_since(last_used).ok()?;
            (unused >= LRU_UNUSED).then(|| MasterCandidate {
                path: path.clone(),
                bytes: metadata.len(),
                last_used,
                referenced: label(path).is_some_and(|label| referenced.contains(label)),
            })
        })
        .collect();
    candidates.sort_by_key(|candidate| (candidate.referenced, candidate.last_used));
    candidates
}

fn recorded(ctx: &DatabaseContext) -> Option<Vec<RecordedMasterFile>> {
    // A connection of its own: this runs on a background thread and must not
    // hold the shared request connection.
    let connection =
        crate::server::database_context::open_scheduler_connection(&ctx.database_path).ok()?;
    match calibration::recorded_master_files(&connection) {
        Ok(records) => Some(records),
        Err(error) => {
            tracing::warn!(db = %ctx.id, "Not cleaning calibration masters: {error}");
            None
        }
    }
}

/// Every master file name any stack index of this database names: the
/// labels a stack's calibration provenance shows. Read as text, so every
/// place an index records one counts, whatever its shape.
pub fn referenced_labels(stack_root: &Path) -> HashSet<String> {
    let mut labels = HashSet::new();
    let folders = [
        storage::stacks(stack_root),
        storage::stack_folder(stack_root, storage::stack_kind::COLOR),
    ];
    for folder in folders {
        for entry in std::fs::read_dir(folder).into_iter().flatten().flatten() {
            let named_index = entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.ends_with(".json"));
            let Ok(text) = named_index
                .then(|| std::fs::read_to_string(entry.path()))
                .transpose()
            else {
                continue;
            };
            labels.extend(text.as_deref().into_iter().flat_map(labels_in));
        }
    }
    labels
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

    #[test]
    fn the_sweep_removes_debris_and_replaced_masters_and_keeps_what_is_needed() {
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
        let label = |fill: char| format!("dark-{}.fits", fill.to_string().repeat(64));
        let master = |fill: char| folder.join(label(fill));
        {
            let connection = ctx.db();
            let connection = connection.lock().unwrap();
            crate::calibration::ensure_schema(&connection).unwrap();
            connection
                .execute(
                    "INSERT INTO psf_guard_calibration_frame
                        (frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                         added_at, updated_at)
                     VALUES ('present', 'rig', 'dark', ?1, 'f', 0, 0)",
                    [frame.to_string_lossy()],
                )
                .unwrap();
            // One identity: a, b, c and d replaced by the newest, e.
            for (fill, created, sources) in [
                ('a', 1, r#"["present"]"#),
                ('b', 2, r#"["present"]"#),
                ('c', 3, r#"["gone"]"#),
                ('d', 4, r#"["present"]"#),
                ('e', 5, r#"["present"]"#),
            ] {
                connection
                    .execute(
                        "INSERT INTO psf_guard_calibration_master
                            (master_uuid, rig_uuid, kind, cache_path, source_set_hash,
                             source_count, source_frame_uuids, created_at, seiza_version,
                             cache_version, exposure_s, camera_temp, statistics_json)
                         VALUES (?1, 'rig', 'dark', ?2, 'hash', 1, ?3, ?4, 'v', 1, 300.0,
                                 -10.0, '{}')",
                        rusqlite::params![
                            fill.to_string(),
                            master(fill).to_string_lossy(),
                            sources,
                            created
                        ],
                    )
                    .unwrap();
            }
        }
        for fill in ['a', 'b', 'c', 'd', 'e'] {
            std::fs::write(master(fill), "master").unwrap();
            aged(&master(fill), 60);
        }
        aged(&master('d'), 3); // used recently
                               // A kept stack names b.
        let stacks = storage::stacks(&ctx.stack_root);
        std::fs::create_dir_all(&stacks).unwrap();
        std::fs::write(
            stacks.join("latest-project-1.json"),
            format!(r#"{{"dark_master":"{}"}}"#, label('b')),
        )
        .unwrap();
        let tmp = folder.join(format!("{}.tmp-1-2", label('f')));
        let orphan_old = master('9');
        let orphan_new = master('8');
        for path in [&tmp, &orphan_old, &orphan_new] {
            std::fs::write(path, "debris").unwrap();
        }
        aged(&tmp, 2);
        aged(&orphan_old, 2);

        let removed = sweep(&ctx, SystemTime::now());

        assert_eq!(removed, 3);
        assert!(!tmp.exists());
        assert!(!orphan_old.exists());
        assert!(orphan_new.exists(), "a day of grace");
        assert!(
            !master('a').exists(),
            "replaced, unused, rebuildable, unnamed"
        );
        assert!(master('b').exists(), "a stack names it");
        assert!(master('c').exists(), "its frames are gone");
        assert!(master('d').exists(), "used recently");
        assert!(master('e').exists(), "the newest");

        let lru: Vec<PathBuf> = lru_candidates(&ctx, SystemTime::now())
            .into_iter()
            .map(|candidate| candidate.path)
            .collect();
        // Rebuildable and unused a week; the one a stack names last.
        assert_eq!(lru.last(), Some(&master('b')));
        assert!(lru.contains(&master('e')));
        assert!(!lru.contains(&master('c')));
        assert!(!lru.contains(&master('d')));
    }
}
