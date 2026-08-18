//! Append-only record of every remote sync action.
//!
//! The remote protocol lets a holder of one bearer token write into the user's
//! scheduler database from off the machine. When a grade or a plan later looks
//! wrong, the operator needs to be able to answer "did a remote client do
//! this, and when" without having kept a terminal open. Console logging alone
//! cannot answer that, so each action also lands in a JSONL file under the
//! cache root.
//!
//! Every action is recorded, including the ones that were turned away: a run
//! of rejected applies is exactly the shape a stolen token leaves behind.
//! Entries carry no token, no path, and no row contents — only which catalog,
//! which operation, and what it did.

use serde::Serialize;
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Roll the log once it passes this size, keeping one previous generation.
/// Bounds the audit trail at twice this without a cron job or a config knob.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Capabilities,
    Export,
    Preview,
    PreviewRefresh,
    Apply,
    Pair,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditOutcome {
    /// The action ran and changed or reported what the client asked for.
    Ok,
    /// The server refused on purpose: a stale preview, an unknown ID, a bundle
    /// that failed validation.
    Refused,
    /// The action broke for an operational reason.
    Failed,
}

#[derive(Debug, Serialize)]
struct AuditEntry<'a> {
    at: String,
    catalog_id: &'a str,
    action: AuditAction,
    outcome: AuditOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    operation: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bundle_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    preview_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<&'a str>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    summary: BTreeMap<String, i64>,
}

/// One remote action, described well enough to reconstruct later.
#[derive(Debug, Default)]
pub struct AuditRecord<'a> {
    pub operation: Option<&'a str>,
    pub source_id: Option<&'a str>,
    pub bundle_id: Option<&'a str>,
    pub preview_id: Option<&'a str>,
    pub detail: Option<&'a str>,
    pub summary: BTreeMap<String, i64>,
}

pub struct RemoteAuditLog {
    path: PathBuf,
    max_bytes: u64,
    /// Serializes appends so two concurrent applies cannot interleave a line.
    write_lock: Mutex<()>,
}

impl RemoteAuditLog {
    pub fn new(cache_root: impl AsRef<Path>) -> Self {
        Self {
            path: cache_root.as_ref().join("remote-sync-audit.jsonl"),
            max_bytes: MAX_LOG_BYTES,
            write_lock: Mutex::new(()),
        }
    }

    /// A smaller roll threshold, so a test can prove rotation without writing
    /// megabytes of filler.
    #[cfg(test)]
    fn with_max_bytes(cache_root: impl AsRef<Path>, max_bytes: u64) -> Self {
        Self {
            max_bytes,
            ..Self::new(cache_root)
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn record(
        &self,
        catalog_id: &str,
        action: AuditAction,
        outcome: AuditOutcome,
        record: AuditRecord<'_>,
    ) {
        let entry = AuditEntry {
            at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            catalog_id,
            action,
            outcome,
            operation: record.operation,
            source_id: record.source_id,
            bundle_id: record.bundle_id,
            preview_id: record.preview_id,
            detail: record.detail,
            summary: record.summary,
        };
        tracing::info!(
            "remote sync {:?} {:?} for db={} operation={} detail={}",
            entry.action,
            entry.outcome,
            catalog_id,
            entry.operation.unwrap_or("-"),
            entry.detail.unwrap_or("-")
        );
        if let Err(error) = self.append(&entry) {
            // The audit file is a record, not a gate. Losing a line must not
            // fail the request the operator is waiting on, but it should be
            // loud enough to notice in the log.
            tracing::warn!(
                "could not write the remote sync audit log {}: {error}",
                self.path.display()
            );
        }
    }

    fn append(&self, entry: &AuditEntry<'_>) -> std::io::Result<()> {
        let mut line = serde_json::to_vec(entry)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        line.push(b'\n');
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent)?;
        }
        self.roll_if_large();
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;
        file.write_all(&line)?;
        Ok(())
    }

    fn roll_if_large(&self) {
        let Ok(metadata) = fs::metadata(&self.path) else {
            return;
        };
        if metadata.len() < self.max_bytes {
            return;
        }
        let _ = fs::rename(&self.path, self.path.with_extension("jsonl.1"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn every_outcome_lands_on_its_own_line() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path().join("cache"));
        log.record(
            "catalog",
            AuditAction::Apply,
            AuditOutcome::Ok,
            AuditRecord {
                operation: Some("push_planning"),
                summary: BTreeMap::from([("total_updated".into(), 5)]),
                ..Default::default()
            },
        );
        log.record(
            "catalog",
            AuditAction::Apply,
            AuditOutcome::Refused,
            AuditRecord {
                operation: Some("push_planning"),
                detail: Some("destination changed"),
                ..Default::default()
            },
        );

        let contents = fs::read_to_string(log.path()).unwrap();
        let lines: Vec<_> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "{contents}");
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["outcome"], "ok");
        assert_eq!(first["summary"]["total_updated"], 5);
        let second: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(second["outcome"], "refused");
        assert_eq!(second["detail"], "destination changed");
        // A refused action has nothing to summarize; the key stays off the wire.
        assert!(second.get("summary").is_none(), "{second}");
    }

    #[test]
    fn a_full_log_rolls_to_one_previous_generation() {
        const FULL: u64 = 64;
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::with_max_bytes(directory.path(), FULL);
        fs::write(log.path(), vec![b'x'; FULL as usize]).unwrap();
        log.record(
            "catalog",
            AuditAction::Export,
            AuditOutcome::Ok,
            AuditRecord::default(),
        );

        let rolled = log.path().with_extension("jsonl.1");
        assert_eq!(fs::metadata(&rolled).unwrap().len(), FULL);
        let contents = fs::read_to_string(log.path()).unwrap();
        assert_eq!(contents.lines().count(), 1, "{contents}");
    }
}
