//! Durable, server-owned previews for database sync.
//!
//! Each preview owns an online SQLite snapshot of its source plus a logical
//! destination fingerprint. Apply reads the snapshot and verifies the
//! destination after taking SQLite's write lock, so it writes the same source
//! data the user reviewed or refuses a stale destination.

use crate::server::api::{SchedulerSyncKind, SchedulerSyncRequest, SchedulerSyncResponse};
use anyhow::{Context, Result};
use rusqlite::backup::Backup;
use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const PREVIEW_LIFETIME: Duration = Duration::from_secs(30 * 60);

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
            let source = Connection::open_with_flags(source_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                .with_context(|| {
                    format!("opening {} for transfer snapshot", source_path.display())
                })?;
            let mut destination = Connection::open(&temporary)
                .with_context(|| format!("creating transfer snapshot {}", temporary.display()))?;
            {
                let backup = Backup::new(&source, &mut destination)
                    .context("starting transfer source snapshot")?;
                backup
                    .run_to_completion(256, Duration::from_millis(10), None)
                    .context("copying transfer source snapshot")?;
            }
            drop(destination);
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
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
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
