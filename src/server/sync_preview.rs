//! Durable, server-owned previews for database sync.
//!
//! The first version records the exact request plus logical source and
//! destination fingerprints. Apply refuses a preview when either catalog has
//! changed. A later transfer-bundle phase will freeze the source rows too, so
//! source changes can wait for the next transfer instead of making a preview
//! stale.

use crate::server::api::{SchedulerSyncRequest, SchedulerSyncResponse};
use anyhow::{Context, Result};
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
    pub source_fingerprint: String,
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
        source_fingerprint: String,
        destination_fingerprint: String,
        result: SchedulerSyncResponse,
    ) -> Result<SyncPreviewRecord> {
        let created_at = unix_seconds();
        let record = SyncPreviewRecord {
            id: Uuid::new_v4().to_string(),
            local_db_id,
            request,
            source_fingerprint,
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
}

pub fn database_fingerprint(path: &Path, tables: &[&str]) -> Result<String> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening {} for sync fingerprint", path.display()))?;
    let transaction = connection
        .unchecked_transaction()
        .context("starting sync fingerprint snapshot")?;
    let mut hasher = Sha256::new();

    for table in tables {
        hash_part(&mut hasher, table.as_bytes());
        let mut statement = transaction
            .prepare(&format!("SELECT * FROM \"{table}\" ORDER BY rowid"))
            .with_context(|| format!("reading {table} for sync fingerprint"))?;
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

    transaction.rollback()?;
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
        let record = manager
            .store(
                "source".into(),
                request(),
                "source-fingerprint".into(),
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

        let before = database_fingerprint(&path, &["acquiredimage"]).unwrap();
        let connection = Connection::open(&path).unwrap();
        connection
            .execute("UPDATE acquiredimage SET gradingStatus = 2", [])
            .unwrap();
        drop(connection);
        let after = database_fingerprint(&path, &["acquiredimage"]).unwrap();

        assert_ne!(before, after);
    }
}
