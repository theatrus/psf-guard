//! Durable, server-owned previews for database sync.
//!
//! Each preview owns an online SQLite snapshot of its source plus a logical
//! destination fingerprint. Apply reads the snapshot and verifies the
//! destination after taking SQLite's write lock, so it writes the same source
//! data the user reviewed or refuses a stale destination.

use crate::server::api::{SchedulerSyncKind, SchedulerSyncRequest, SchedulerSyncResponse};
use crate::server::database_context::{
    open_scheduler_connection_with_flags, SCHEDULER_BUSY_TIMEOUT,
};
use anyhow::{Context, Result};
use rusqlite::backup::{Backup, StepResult};
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const PREVIEW_LIFETIME: Duration = Duration::from_secs(30 * 60);
/// A busy source gets the normal scheduler lock grace period, but even a
/// source that changes often enough to keep restarting SQLite's online backup
/// must eventually yield a result or an actionable failure.
const SNAPSHOT_OVERALL_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The online backup API reports Busy/Locked as retry states rather than
/// errors. Preserve exhausted retry deadlines as a typed operational failure
/// so the HTTP layer does not mislabel source contention as bad input.
#[derive(Debug)]
pub(crate) struct SyncSnapshotContention {
    pub(crate) message: String,
}

impl std::fmt::Display for SyncSnapshotContention {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for SyncSnapshotContention {}

pub(crate) fn error_is_sync_snapshot_contention(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<SyncSnapshotContention>().is_some())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncPreviewRecord {
    pub id: String,
    pub local_db_id: String,
    pub request: SchedulerSyncRequest,
    pub source_snapshot_file: String,
    pub destination_fingerprint: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub result: SchedulerSyncResponse,
}

pub struct SyncPreviewManager {
    directory: PathBuf,
    records: Mutex<HashMap<String, SyncPreviewRecord>>,
}

impl SyncPreviewManager {
    pub fn new(cache_root: impl AsRef<Path>) -> Self {
        let directory = cache_root.as_ref().join("sync-previews");
        let records = load_records(&directory);
        Self {
            directory,
            records: Mutex::new(records),
        }
    }

    pub fn store(
        &self,
        local_db_id: String,
        request: SchedulerSyncRequest,
        source_snapshot_file: String,
        destination_fingerprint: String,
        result: SchedulerSyncResponse,
    ) -> Result<SyncPreviewRecord> {
        let created_at = unix_seconds();
        let record = SyncPreviewRecord {
            id: Uuid::new_v4().to_string(),
            local_db_id,
            request,
            source_snapshot_file,
            destination_fingerprint,
            created_at,
            expires_at: created_at + PREVIEW_LIFETIME.as_secs() as i64,
            result,
        };
        fs::create_dir_all(&self.directory).with_context(|| {
            format!(
                "creating sync preview directory {}",
                self.directory.display()
            )
        })?;
        write_record(&self.directory, &record)?;
        self.records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?
            .insert(record.id.clone(), record.clone());
        Ok(record)
    }

    /// Take a transactionally consistent online copy of a live SQLite source.
    pub fn create_source_snapshot(&self, source_path: &Path) -> Result<String> {
        fs::create_dir_all(&self.directory).with_context(|| {
            format!(
                "creating sync preview directory {}",
                self.directory.display()
            )
        })?;
        let filename = format!("{}.source.sqlite", Uuid::new_v4());
        let published = self.directory.join(&filename);
        let temporary = self.directory.join(format!("{filename}.tmp"));
        let copy = || -> Result<()> {
            copy_source_snapshot(
                source_path,
                &temporary,
                SCHEDULER_BUSY_TIMEOUT,
                SNAPSHOT_OVERALL_TIMEOUT,
            )?;
            fs::rename(&temporary, &published)
                .with_context(|| format!("publishing transfer snapshot {}", published.display()))?;
            Ok(())
        };
        if let Err(error) = copy() {
            let _ = fs::remove_file(&temporary);
            return Err(error);
        }
        Ok(filename)
    }

    /// Reserve a durable snapshot path for a remote bundle materializer.
    pub fn create_empty_source_snapshot(&self) -> Result<String> {
        fs::create_dir_all(&self.directory).with_context(|| {
            format!(
                "creating sync preview directory {}",
                self.directory.display()
            )
        })?;
        Ok(format!("{}.source.sqlite", Uuid::new_v4()))
    }

    pub fn source_snapshot_path(&self, record: &SyncPreviewRecord) -> Result<PathBuf> {
        self.source_snapshot_path_for_file(&record.source_snapshot_file)
    }

    pub fn source_snapshot_path_for_file(&self, filename: &str) -> Result<PathBuf> {
        snapshot_path(&self.directory, filename)
    }

    pub fn remove_source_snapshot(&self, filename: &str) {
        if let Ok(path) = snapshot_path(&self.directory, filename) {
            let _ = fs::remove_file(path);
        }
    }

    pub fn get(&self, id: &str) -> Result<Option<SyncPreviewRecord>> {
        let mut records = self
            .records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?;
        let Some(record) = records.get(id).cloned() else {
            return Ok(None);
        };
        if record.expires_at <= unix_seconds() {
            records.remove(id);
            let _ = fs::remove_file(record_path(&self.directory, id));
            self.remove_source_snapshot(&record.source_snapshot_file);
            return Ok(None);
        }
        if !self.source_snapshot_path(&record)?.is_file() {
            records.remove(id);
            let _ = fs::remove_file(record_path(&self.directory, id));
            return Ok(None);
        }
        Ok(Some(record))
    }

    /// Every unexpired preview staged for one catalog, newest first —
    /// including previews a remote client created, which the UI could not
    /// otherwise discover. Expired records and records whose snapshot file
    /// vanished are dropped on the way, like `get` does.
    pub fn list(&self, local_db_id: &str) -> Result<Vec<SyncPreviewRecord>> {
        let mut records = self
            .records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?;
        let now = unix_seconds();
        let mut dead = Vec::new();
        let mut alive = Vec::new();
        for (id, record) in records.iter() {
            if record.local_db_id != local_db_id {
                continue;
            }
            let snapshot_exists = self
                .source_snapshot_path(record)
                .map(|path| path.is_file())
                .unwrap_or(false);
            if record.expires_at <= now || !snapshot_exists {
                dead.push((id.clone(), record.source_snapshot_file.clone()));
            } else {
                alive.push(record.clone());
            }
        }
        for (id, snapshot) in dead {
            records.remove(&id);
            let _ = fs::remove_file(record_path(&self.directory, &id));
            self.remove_source_snapshot(&snapshot);
        }
        alive.sort_by_key(|record| std::cmp::Reverse(record.created_at));
        Ok(alive)
    }

    /// Atomically take a preview for one Apply attempt. A stale or failed
    /// Apply must be previewed again; two callers can never apply the same ID.
    pub fn claim(&self, id: &str, local_db_id: &str) -> Result<Option<SyncPreviewRecord>> {
        let mut records = self
            .records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?;
        let Some(record) = records.get(id).cloned() else {
            return Ok(None);
        };
        if record.local_db_id != local_db_id {
            return Ok(None);
        }
        if record.expires_at <= unix_seconds() {
            records.remove(id);
            let _ = fs::remove_file(record_path(&self.directory, id));
            self.remove_source_snapshot(&record.source_snapshot_file);
            return Ok(None);
        }
        match fs::remove_file(record_path(&self.directory, id)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("claiming sync preview"),
        }
        records.remove(id);
        Ok(Some(record))
    }

    /// Put a claimed preview back after an apply that changed nothing.
    ///
    /// Claiming is deliberately one-shot, but an apply that refuses (the
    /// destination moved) or breaks (the file was locked) has written nothing,
    /// so throwing the preview away costs the client its whole source snapshot
    /// — for a remote client, a re-upload of the entire bundle. Restoring keeps
    /// it, so the client can retry, or refresh the preview against the
    /// destination as it now stands.
    pub fn restore(&self, record: &SyncPreviewRecord) -> Result<()> {
        if record.expires_at <= unix_seconds() {
            self.remove_source_snapshot(&record.source_snapshot_file);
            return Ok(());
        }
        write_record(&self.directory, record)?;
        self.records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?
            .insert(record.id.clone(), record.clone());
        Ok(())
    }

    /// Re-run a preview in place: same ID, same source snapshot, new summary
    /// and destination fingerprint. Lets a client whose preview went stale
    /// review the merge again without re-sending the source data.
    pub fn refresh(
        &self,
        record: &SyncPreviewRecord,
        destination_fingerprint: String,
        result: SchedulerSyncResponse,
    ) -> Result<SyncPreviewRecord> {
        let refreshed = SyncPreviewRecord {
            destination_fingerprint,
            result,
            ..record.clone()
        };
        write_record(&self.directory, &refreshed)?;
        self.records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?
            .insert(refreshed.id.clone(), refreshed.clone());
        Ok(refreshed)
    }

    pub fn discard(&self, id: &str, local_db_id: &str) -> Result<bool> {
        let mut records = self
            .records
            .lock()
            .map_err(|error| anyhow::anyhow!("sync preview lock poisoned: {error}"))?;
        let Some(record) = records.get(id).cloned() else {
            return Ok(false);
        };
        if record.local_db_id != local_db_id {
            return Ok(false);
        }
        records.remove(id);
        let _ = fs::remove_file(record_path(&self.directory, id));
        self.remove_source_snapshot(&record.source_snapshot_file);
        Ok(true)
    }
}

fn copy_source_snapshot(
    source_path: &Path,
    temporary: &Path,
    stall_timeout: Duration,
    overall_timeout: Duration,
) -> Result<()> {
    let source =
        open_scheduler_connection_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| {
                format!(
                    "opening source scheduler database {}",
                    source_path.display()
                )
            })?;
    // sqlite3_backup_step reports Busy/Locked as successful retry states. Its
    // own loop below owns the deadline, so disable the connection handler that
    // would otherwise wait a full minute inside one step before we can check it.
    source
        .busy_timeout(Duration::ZERO)
        .context("configuring bounded transfer snapshot lock handling")?;
    let mut destination = Connection::open(temporary)
        .with_context(|| format!("creating transfer snapshot {}", temporary.display()))?;
    let backup =
        Backup::new(&source, &mut destination).context("starting transfer source snapshot")?;
    let pause = Duration::from_millis(10);
    let started = Instant::now();
    let mut last_net_progress = started;
    let mut lowest_remaining = None;
    loop {
        let step = backup
            .step(256)
            .context("copying transfer source snapshot")?;
        let now = Instant::now();
        let done = matches!(step, StepResult::Done);
        match step {
            StepResult::Done => {}
            StepResult::More => {
                let remaining = backup.progress().remaining;
                if record_snapshot_progress(&mut lowest_remaining, remaining) {
                    last_net_progress = now;
                }
            }
            StepResult::Busy | StepResult::Locked => {}
            _ => {}
        }

        let progress = backup.progress();
        if now.duration_since(started) >= overall_timeout {
            return Err(SyncSnapshotContention {
                message: format!(
                    "source scheduler database {} did not finish a sync preview snapshot within {} seconds ({} of {} pages remain)",
                    source_path.display(),
                    overall_timeout.as_secs_f64(),
                    progress.remaining,
                    progress.pagecount,
                ),
            }
            .into());
        }
        if done {
            break;
        }
        if now.duration_since(last_net_progress) >= stall_timeout {
            return Err(SyncSnapshotContention {
                message: format!(
                    "source scheduler database {} made no snapshot progress for {} seconds while creating a sync preview ({} of {} pages remain; the database may be locked or changing continuously)",
                    source_path.display(),
                    stall_timeout.as_secs_f64(),
                    progress.remaining,
                    progress.pagecount,
                ),
            }
            .into());
        }
        std::thread::sleep(pause);
    }
    Ok(())
}

/// SQLite restarts an online backup when another connection changes the
/// source. A successful step after that restart is not net progress if it only
/// reaches a page count we already copied, so retain the lowest remaining-page
/// count instead of treating every `StepResult::More` as progress.
fn record_snapshot_progress(lowest_remaining: &mut Option<i32>, remaining: i32) -> bool {
    match lowest_remaining {
        Some(lowest) if remaining >= *lowest => false,
        slot => {
            *slot = Some(remaining);
            true
        }
    }
}

#[derive(Clone, Copy)]
pub struct FingerprintQuery {
    label: &'static str,
    sql: &'static str,
}

pub fn fingerprint_queries(request: &SchedulerSyncRequest) -> Vec<FingerprintQuery> {
    const EXPOSURE_TEMPLATE: FingerprintQuery = FingerprintQuery {
        label: "exposuretemplate",
        sql: "SELECT * FROM exposuretemplate ORDER BY rowid",
    };
    const PROJECT: FingerprintQuery = FingerprintQuery {
        label: "project",
        sql: "SELECT * FROM project ORDER BY rowid",
    };
    const RULE_WEIGHT: FingerprintQuery = FingerprintQuery {
        label: "ruleweight",
        sql: "SELECT * FROM ruleweight ORDER BY rowid",
    };
    const TARGET: FingerprintQuery = FingerprintQuery {
        label: "target",
        sql: "SELECT * FROM target ORDER BY rowid",
    };
    const EXPOSURE_PLAN: FingerprintQuery = FingerprintQuery {
        label: "exposureplan",
        sql: "SELECT * FROM exposureplan ORDER BY rowid",
    };
    const ACQUIRED_IMAGE: FingerprintQuery = FingerprintQuery {
        label: "acquiredimage",
        sql: "SELECT * FROM acquiredimage ORDER BY rowid",
    };
    const IMAGE_DATA_KEYS: FingerprintQuery = FingerprintQuery {
        label: "imagedata-keys",
        sql: "SELECT acquiredimageid, tag FROM imagedata \
              ORDER BY acquiredimageid, tag, Id",
    };
    const GRADES: FingerprintQuery = FingerprintQuery {
        label: "grades",
        sql: "SELECT guid, gradingStatus, rejectreason FROM acquiredimage ORDER BY guid, Id",
    };

    match request.kind {
        SchedulerSyncKind::Pull => {
            let mut queries = vec![
                EXPOSURE_TEMPLATE,
                PROJECT,
                RULE_WEIGHT,
                TARGET,
                EXPOSURE_PLAN,
                ACQUIRED_IMAGE,
            ];
            if request.with_image_data.unwrap_or(true) {
                queries.push(IMAGE_DATA_KEYS);
            }
            queries
        }
        SchedulerSyncKind::PushPlanning => vec![
            EXPOSURE_TEMPLATE,
            PROJECT,
            RULE_WEIGHT,
            TARGET,
            EXPOSURE_PLAN,
        ],
        SchedulerSyncKind::PushGrades => vec![GRADES],
    }
}

pub fn database_fingerprint(path: &Path, queries: &[FingerprintQuery]) -> Result<String> {
    let connection = open_scheduler_connection_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {} for sync fingerprint", path.display()))?;
    connection_fingerprint(&connection, queries)
}

pub fn connection_fingerprint(
    connection: &Connection,
    queries: &[FingerprintQuery],
) -> Result<String> {
    let mut hasher = Sha256::new();

    for query in queries {
        hash_part(&mut hasher, query.label.as_bytes());
        let mut statement = connection
            .prepare(query.sql)
            .with_context(|| format!("reading {} for sync fingerprint", query.label))?;
        for column in statement.column_names() {
            hash_part(&mut hasher, column.as_bytes());
        }
        let column_count = statement.column_count();
        let mut rows = statement.query([])?;
        while let Some(row) = rows.next()? {
            hasher.update([0xff]);
            for index in 0..column_count {
                match row.get_ref(index)? {
                    ValueRef::Null => hasher.update([0]),
                    ValueRef::Integer(value) => {
                        hasher.update([1]);
                        hasher.update(value.to_le_bytes());
                    }
                    ValueRef::Real(value) => {
                        hasher.update([2]);
                        hasher.update(value.to_bits().to_le_bytes());
                    }
                    ValueRef::Text(value) => {
                        hasher.update([3]);
                        hash_part(&mut hasher, value);
                    }
                    ValueRef::Blob(value) => {
                        hasher.update([4]);
                        hash_part(&mut hasher, value);
                    }
                }
            }
        }
    }

    let mut fingerprint = String::with_capacity(64);
    for byte in hasher.finalize() {
        write!(&mut fingerprint, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(fingerprint)
}

fn hash_part(hasher: &mut Sha256, value: &[u8]) {
    hasher.update((value.len() as u64).to_le_bytes());
    hasher.update(value);
}

fn unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn load_records(directory: &Path) -> HashMap<String, SyncPreviewRecord> {
    let mut records = HashMap::new();
    let Ok(entries) = fs::read_dir(directory) else {
        return records;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<SyncPreviewRecord>(&bytes) else {
            continue;
        };
        if record.expires_at > unix_seconds() {
            records.insert(record.id.clone(), record);
        } else {
            let _ = fs::remove_file(path);
            if let Ok(snapshot) = snapshot_path(directory, &record.source_snapshot_file) {
                let _ = fs::remove_file(snapshot);
            }
        }
    }
    records
}

fn write_record(directory: &Path, record: &SyncPreviewRecord) -> Result<()> {
    let path = record_path(directory, &record.id);
    let temporary = path.with_extension("json.tmp");
    let bytes = serde_json::to_vec_pretty(record)?;
    fs::write(&temporary, bytes)
        .with_context(|| format!("writing sync preview {}", temporary.display()))?;
    fs::rename(&temporary, &path)
        .with_context(|| format!("publishing sync preview {}", path.display()))?;
    Ok(())
}

fn record_path(directory: &Path, id: &str) -> PathBuf {
    directory.join(format!("{id}.json"))
}

fn snapshot_path(directory: &Path, filename: &str) -> Result<PathBuf> {
    let name = Path::new(filename);
    anyhow::ensure!(
        name.file_name().and_then(|value| value.to_str()) == Some(filename)
            && filename.ends_with(".source.sqlite"),
        "invalid transfer snapshot name"
    );
    Ok(directory.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::api::{
        SchedulerSyncGradeCounts, SchedulerSyncKind, SchedulerSyncTableCounts,
    };
    use tempfile::tempdir;

    fn response() -> SchedulerSyncResponse {
        SchedulerSyncResponse {
            kind: SchedulerSyncKind::PushGrades,
            dry_run: true,
            source_db_id: "source".into(),
            destination_db_id: "destination".into(),
            exposuretemplate: SchedulerSyncTableCounts::default(),
            project: SchedulerSyncTableCounts::default(),
            ruleweight: SchedulerSyncTableCounts::default(),
            target: SchedulerSyncTableCounts::default(),
            exposureplan: SchedulerSyncTableCounts::default(),
            acquiredimage: None,
            imagedata: None,
            grades: Some(SchedulerSyncGradeCounts::default()),
            grade_filled: 0,
            grade_preserved: 0,
            imagedata_bytes: 0,
            total_inserted: 0,
            total_updated: 0,
            changes: Vec::new(),
        }
    }

    fn request() -> SchedulerSyncRequest {
        SchedulerSyncRequest {
            peer_db_id: "destination".into(),
            kind: SchedulerSyncKind::PushGrades,
            dry_run: true,
            with_image_data: None,
            project: None,
            target: None,
            status: None,
            reviewed_only: true,
        }
    }

    #[test]
    fn records_survive_manager_recreation_and_can_be_claimed_once() {
        let directory = tempdir().unwrap();
        let manager = SyncPreviewManager::new(directory.path());
        let source_path = directory.path().join("source.sqlite");
        let source = Connection::open(&source_path).unwrap();
        source
            .execute_batch("CREATE TABLE sample (value TEXT);")
            .unwrap();
        drop(source);
        let snapshot = manager.create_source_snapshot(&source_path).unwrap();
        let record = manager
            .store(
                "source".into(),
                request(),
                snapshot,
                "destination-fingerprint".into(),
                response(),
            )
            .unwrap();
        drop(manager);

        let manager = SyncPreviewManager::new(directory.path());
        assert!(manager.get(&record.id).unwrap().is_some());
        assert!(manager
            .claim(&record.id, "wrong-database")
            .unwrap()
            .is_none());
        assert!(manager.claim(&record.id, "source").unwrap().is_some());
        assert!(manager.claim(&record.id, "source").unwrap().is_none());
        assert!(manager.get(&record.id).unwrap().is_none());
    }

    /// Build a manager holding one preview over a throwaway source database.
    fn stored_preview(directory: &Path) -> (SyncPreviewManager, SyncPreviewRecord) {
        let manager = SyncPreviewManager::new(directory.join("cache"));
        let source_path = directory.join("source.sqlite");
        let source = Connection::open(&source_path).unwrap();
        source
            .execute_batch("CREATE TABLE sample (value TEXT);")
            .unwrap();
        drop(source);
        let snapshot = manager.create_source_snapshot(&source_path).unwrap();
        let record = manager
            .store(
                "catalog".into(),
                request(),
                snapshot,
                "destination-fingerprint".into(),
                response(),
            )
            .unwrap();
        (manager, record)
    }

    #[test]
    fn a_restored_preview_can_be_claimed_again() {
        // An apply that refuses wrote nothing, so its preview is still valid
        // source data. Losing it would cost a remote client the whole upload.
        let directory = tempdir().unwrap();
        let (manager, record) = stored_preview(directory.path());

        let claimed = manager.claim(&record.id, "catalog").unwrap().unwrap();
        assert!(manager.get(&record.id).unwrap().is_none());
        manager.restore(&claimed).unwrap();

        assert!(manager.get(&record.id).unwrap().is_some());
        // Restoring survives a restart, so the record went back to disk too.
        let reloaded = SyncPreviewManager::new(directory.path().join("cache"));
        assert!(reloaded.get(&record.id).unwrap().is_some());
        assert!(manager.claim(&record.id, "catalog").unwrap().is_some());
    }

    #[test]
    fn restoring_an_expired_preview_drops_its_snapshot_instead() {
        let directory = tempdir().unwrap();
        let (manager, record) = stored_preview(directory.path());
        let snapshot = manager.source_snapshot_path(&record).unwrap();
        let expired = SyncPreviewRecord {
            expires_at: unix_seconds() - 1,
            ..manager.claim(&record.id, "catalog").unwrap().unwrap()
        };

        manager.restore(&expired).unwrap();

        assert!(manager.get(&record.id).unwrap().is_none());
        assert!(
            !snapshot.exists(),
            "an expired preview must not leak a file"
        );
    }

    #[test]
    fn refreshing_keeps_the_id_and_snapshot_but_takes_the_new_fingerprint() {
        // The way back from a stale preview: re-review the same source data
        // against the destination as it now stands.
        let directory = tempdir().unwrap();
        let (manager, record) = stored_preview(directory.path());

        let refreshed = manager
            .refresh(&record, "moved-destination".into(), response())
            .unwrap();

        assert_eq!(refreshed.id, record.id);
        assert_eq!(refreshed.source_snapshot_file, record.source_snapshot_file);
        assert_eq!(refreshed.destination_fingerprint, "moved-destination");
        let reloaded = SyncPreviewManager::new(directory.path().join("cache"));
        assert_eq!(
            reloaded
                .get(&record.id)
                .unwrap()
                .unwrap()
                .destination_fingerprint,
            "moved-destination"
        );
    }

    #[test]
    fn an_expired_preview_is_dropped_with_its_snapshot_on_load() {
        let directory = tempdir().unwrap();
        let (manager, record) = stored_preview(directory.path());
        let snapshot = manager.source_snapshot_path(&record).unwrap();
        let expired = SyncPreviewRecord {
            expires_at: unix_seconds() - 1,
            ..record.clone()
        };
        write_record(
            &directory.path().join("cache").join("sync-previews"),
            &expired,
        )
        .unwrap();

        let reloaded = SyncPreviewManager::new(directory.path().join("cache"));

        assert!(reloaded.get(&record.id).unwrap().is_none());
        assert!(
            !snapshot.exists(),
            "an expired preview must not leak a file"
        );
    }

    #[test]
    fn source_snapshot_keeps_the_previewed_rows() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let source = Connection::open(&source_path).unwrap();
        source
            .execute_batch(
                "CREATE TABLE sample (value TEXT);
                 INSERT INTO sample VALUES ('previewed');",
            )
            .unwrap();
        drop(source);

        let manager = SyncPreviewManager::new(directory.path().join("cache"));
        let filename = manager.create_source_snapshot(&source_path).unwrap();
        let source = Connection::open(&source_path).unwrap();
        source
            .execute("UPDATE sample SET value = 'later'", [])
            .unwrap();

        let snapshot =
            Connection::open(manager.source_snapshot_path_for_file(&filename).unwrap()).unwrap();
        let value: String = snapshot
            .query_row("SELECT value FROM sample", [], |row| row.get(0))
            .unwrap();
        assert_eq!(value, "previewed");
    }

    #[test]
    fn source_snapshot_stops_waiting_for_a_persistent_lock() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let temporary = directory.path().join("snapshot.tmp");
        let locker = Connection::open(&source_path).unwrap();
        locker
            .execute_batch("CREATE TABLE sample (value TEXT); BEGIN EXCLUSIVE;")
            .unwrap();

        let started = Instant::now();
        let error = copy_source_snapshot(
            &source_path,
            &temporary,
            Duration::from_millis(25),
            Duration::from_secs(1),
        )
        .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(format!("{error:#}").contains("made no snapshot progress"));
        locker.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn source_snapshot_has_a_hard_overall_deadline() {
        let directory = tempdir().unwrap();
        let source_path = directory.path().join("source.sqlite");
        let temporary = directory.path().join("snapshot.tmp");
        let locker = Connection::open(&source_path).unwrap();
        locker
            .execute_batch("CREATE TABLE sample (value TEXT); BEGIN EXCLUSIVE;")
            .unwrap();

        let started = Instant::now();
        let error = copy_source_snapshot(
            &source_path,
            &temporary,
            Duration::from_secs(1),
            Duration::from_millis(25),
        )
        .unwrap_err();

        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(format!("{error:#}").contains("did not finish"));
        locker.execute_batch("ROLLBACK").unwrap();
    }

    #[test]
    fn backup_restarts_do_not_count_as_net_progress() {
        let mut lowest_remaining = None;
        assert!(record_snapshot_progress(&mut lowest_remaining, 744));
        assert!(!record_snapshot_progress(&mut lowest_remaining, 1_000));
        assert!(!record_snapshot_progress(&mut lowest_remaining, 744));
        assert!(record_snapshot_progress(&mut lowest_remaining, 743));
        assert_eq!(lowest_remaining, Some(743));
    }

    #[test]
    fn logical_fingerprint_changes_with_a_row() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("catalog.sqlite");
        let connection = Connection::open(&path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, gradingStatus INTEGER);
                 INSERT INTO acquiredimage VALUES (1, 0);",
            )
            .unwrap();
        drop(connection);

        let query = FingerprintQuery {
            label: "acquiredimage",
            sql: "SELECT * FROM acquiredimage ORDER BY rowid",
        };
        let before = database_fingerprint(&path, &[query]).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("UPDATE acquiredimage SET gradingStatus = 2", [])
            .unwrap();
        drop(connection);
        let after = database_fingerprint(&path, &[query]).unwrap();

        assert_ne!(before, after);
    }
}
