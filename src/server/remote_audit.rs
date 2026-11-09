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

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

/// Roll the log once it passes this size, keeping one previous generation.
/// Bounds the audit trail at twice this without a cron job or a config knob.
const MAX_LOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuditAction {
    Capabilities,
    Export,
    Preview,
    PreviewRefresh,
    Apply,
    Pair,
    FlatHistorySnapshot,
    FlatHistoryPending,
    FlatHistoryAcknowledge,
    StarDetectionProfile,
    /// An image file sent to the database's receive folder.
    ImageUpload,
    /// A report of what N.I.N.A.'s sequence is doing. Only refusals reach
    /// the log; accepted reports arrive every few seconds.
    SequenceStatus,
}

impl AuditAction {
    /// Whether the Live page shows this as a client's latest action. A
    /// capabilities call and a poll for pending flat history run before
    /// nearly everything else, so they count only as contact.
    fn is_notable(self) -> bool {
        !matches!(
            self,
            Self::Capabilities | Self::FlatHistoryPending | Self::SequenceStatus
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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

/// What a Sync client last did to one catalog: the newest notable entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LatestAction {
    /// Unix milliseconds.
    pub at_ms: i64,
    pub action: AuditAction,
    pub outcome: AuditOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub summary: BTreeMap<String, i64>,
}

/// A line read back from the log.
#[derive(Deserialize)]
struct LoggedEntry {
    at: String,
    catalog_id: String,
    action: AuditAction,
    outcome: AuditOutcome,
    #[serde(default)]
    operation: Option<String>,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    summary: BTreeMap<String, i64>,
}

/// How much of the end of the log to read back after a restart.
const LATEST_TAIL_BYTES: u64 = 512 * 1024;

pub struct RemoteAuditLog {
    path: PathBuf,
    max_bytes: u64,
    /// Serializes appends so two concurrent applies cannot interleave a line.
    write_lock: Mutex<()>,
    /// Catalog ID to its newest notable entry. `None` until first used, then
    /// read back from the end of the log, so a restart keeps it.
    latest: Mutex<Option<HashMap<String, LatestAction>>>,
}

impl RemoteAuditLog {
    /// Where the log sits below a cache folder.
    pub fn path_in(cache_root: impl AsRef<Path>) -> PathBuf {
        cache_root.as_ref().join("remote-sync-audit.jsonl")
    }

    pub fn new(cache_root: impl AsRef<Path>) -> Self {
        Self {
            path: Self::path_in(cache_root),
            max_bytes: MAX_LOG_BYTES,
            write_lock: Mutex::new(()),
            latest: Mutex::new(None),
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
        let now = chrono::Utc::now();
        if action.is_notable() {
            self.remember(
                catalog_id,
                LatestAction {
                    at_ms: now.timestamp_millis(),
                    action,
                    outcome,
                    operation: record.operation.map(str::to_owned),
                    detail: record.detail.map(str::to_owned),
                    summary: record.summary.clone(),
                },
            );
        }
        let entry = AuditEntry {
            at: now.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
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

    /// The newest notable action on `catalog_id`, if the log has one.
    pub fn latest(&self, catalog_id: &str) -> Option<LatestAction> {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest
            .get_or_insert_with(|| self.read_latest())
            .get(catalog_id)
            .cloned()
    }

    fn remember(&self, catalog_id: &str, action: LatestAction) {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest
            .get_or_insert_with(|| self.read_latest())
            .insert(catalog_id.to_owned(), action);
    }

    /// The newest notable entry per catalog in the end of the log and, when
    /// the log rolled recently, the end of the previous generation.
    fn read_latest(&self) -> HashMap<String, LatestAction> {
        let mut latest = HashMap::new();
        for path in [self.path.with_extension("jsonl.1"), self.path.clone()] {
            let Ok(tail) = read_tail(&path, LATEST_TAIL_BYTES) else {
                continue;
            };
            for line in tail.lines() {
                let Ok(entry) = serde_json::from_str::<LoggedEntry>(line) else {
                    continue;
                };
                if !entry.action.is_notable() {
                    continue;
                }
                let Ok(at) = chrono::DateTime::parse_from_rfc3339(&entry.at) else {
                    continue;
                };
                latest.insert(
                    entry.catalog_id,
                    LatestAction {
                        at_ms: at.timestamp_millis(),
                        action: entry.action,
                        outcome: entry.outcome,
                        operation: entry.operation,
                        detail: entry.detail,
                        summary: entry.summary,
                    },
                );
            }
        }
        latest
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

/// The last `limit` bytes of `path`, from the first whole line.
fn read_tail(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(limit);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(if start == 0 {
        text.into_owned()
    } else {
        // The first line was cut; skip it.
        text.split_once('\n')
            .map(|(_, rest)| rest.to_owned())
            .unwrap_or_default()
    })
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
    fn the_latest_notable_action_survives_a_restart() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        let record = |log: &RemoteAuditLog, catalog, action, operation| {
            log.record(
                catalog,
                action,
                AuditOutcome::Ok,
                AuditRecord {
                    operation,
                    summary: BTreeMap::from([("total_updated".into(), 2)]),
                    ..Default::default()
                },
            )
        };
        record(&log, "rig", AuditAction::Apply, Some("merge"));
        // Contact only: a capabilities call never replaces the latest action.
        record(&log, "rig", AuditAction::Capabilities, None);
        record(&log, "other", AuditAction::Export, Some("push_planning"));
        let latest = log.latest("rig").unwrap();
        assert_eq!(
            (latest.action, latest.operation.as_deref()),
            (AuditAction::Apply, Some("merge"))
        );
        assert_eq!(latest.summary["total_updated"], 2);
        assert!(log.latest("missing").is_none());

        let reopened = RemoteAuditLog::new(directory.path());
        assert_eq!(
            reopened.latest("rig").map(|latest| latest.action),
            Some(AuditAction::Apply)
        );
        assert_eq!(
            reopened.latest("other").and_then(|latest| latest.operation),
            Some("push_planning".into())
        );
    }

    #[test]
    fn reading_back_skips_a_cut_first_line_and_broken_lines() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        let line = r#"{"at":"2026-11-08T10:00:00Z","catalog_id":"rig","action":"image_upload","outcome":"ok","detail":"M31_0001.fits"}"#;
        let filler = format!("{}\n", "x".repeat(LATEST_TAIL_BYTES as usize));
        fs::write(log.path(), format!("{filler}not json\n{line}\n")).unwrap();
        let latest = log.latest("rig").unwrap();
        assert_eq!(latest.action, AuditAction::ImageUpload);
        assert_eq!(latest.detail.as_deref(), Some("M31_0001.fits"));
        assert_eq!(latest.at_ms, 1_794_132_000_000);
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
