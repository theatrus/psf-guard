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
    /// An image file a paired Director plugin sent to the receive folder.
    DirectorImageUpload,
    /// A report of what N.I.N.A.'s sequence is doing. Only refusals reach
    /// the log; accepted reports arrive every few seconds.
    SequenceStatus,
}

impl AuditAction {
    /// An image file sent in, rather than a sync exchange.
    fn is_upload(self) -> bool {
        matches!(self, Self::ImageUpload | Self::DirectorImageUpload)
    }

    /// Whether a Sync client took this action, rather than a Director one.
    fn is_sync(self) -> bool {
        self != Self::DirectorImageUpload
    }

    /// Whether the Live page can show this as a client's action. A
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

/// How much of the end of the log to read back when there is no saved record
/// of the latest actions yet, as on the first start after an upgrade.
const LATEST_TAIL_BYTES: u64 = 512 * 1024;

/// How much of the end of each log generation the history reads.
const HISTORY_TAIL_BYTES: u64 = 2 * 1024 * 1024;

/// Log a refused sequence report at most this often per catalog. A plugin
/// whose key was revoked keeps sending them every 15 s.
const SEQUENCE_REFUSAL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Catalog ID, then a kind of action (its action and operation), to the
/// newest entry of that kind.
type LatestByKind = HashMap<String, BTreeMap<String, LatestAction>>;

#[derive(Default)]
struct LatestState {
    by_catalog: LatestByKind,
    /// Bumped for each snapshot sent to the file.
    generation: u64,
}

pub struct RemoteAuditLog {
    path: PathBuf,
    max_bytes: u64,
    /// Serializes appends so two concurrent applies cannot interleave a line.
    write_lock: Mutex<()>,
    /// The newest notable entry of each kind, per catalog. Loaded when the
    /// server starts and saved beside the log, so a restart keeps it however
    /// much the log grows.
    latest: Mutex<LatestState>,
    latest_path: PathBuf,
    /// Serializes snapshot writes, and records the generation last written so
    /// an older snapshot never replaces a newer one.
    latest_written: std::sync::Arc<Mutex<u64>>,
    /// When each catalog last had a refused sequence report logged.
    sequence_refusals: Mutex<HashMap<String, std::time::Instant>>,
}

impl RemoteAuditLog {
    /// Where the log sits below a cache folder.
    pub fn path_in(cache_root: impl AsRef<Path>) -> PathBuf {
        cache_root.as_ref().join("remote-sync-audit.jsonl")
    }

    /// Where the latest actions are saved below a cache folder.
    pub fn latest_path_in(cache_root: impl AsRef<Path>) -> PathBuf {
        cache_root.as_ref().join("remote-sync-latest.json")
    }

    /// Reads the saved latest actions, or the end of the log when none are
    /// saved. Called while the server starts, never on a request.
    pub fn new(cache_root: impl AsRef<Path>) -> Self {
        let path = Self::path_in(&cache_root);
        let latest_path = Self::latest_path_in(&cache_root);
        let by_catalog = fs::read(&latest_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_else(|| read_latest_from_log(&path));
        Self {
            path,
            max_bytes: MAX_LOG_BYTES,
            write_lock: Mutex::new(()),
            latest: Mutex::new(LatestState {
                by_catalog,
                generation: 0,
            }),
            latest_path,
            latest_written: std::sync::Arc::default(),
            sequence_refusals: Mutex::default(),
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
        if action == AuditAction::SequenceStatus
            && outcome != AuditOutcome::Ok
            && !self.sequence_refusal_due(catalog_id)
        {
            tracing::debug!("remote sync sequence report {outcome:?} for db={catalog_id}");
            return;
        }
        let now = chrono::Utc::now();
        if is_notable(action, record.detail) {
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

    fn sequence_refusal_due(&self, catalog_id: &str) -> bool {
        let mut refusals = self
            .sequence_refusals
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let now = std::time::Instant::now();
        match refusals.get(catalog_id) {
            Some(at) if now.duration_since(*at) < SEQUENCE_REFUSAL_INTERVAL => false,
            _ => {
                refusals.insert(catalog_id.to_owned(), now);
                true
            }
        }
    }

    /// The Sync plugin's newest notable action on `catalog_id`, if any.
    pub fn latest(&self, catalog_id: &str) -> Option<LatestAction> {
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest
            .by_catalog
            .get(catalog_id)?
            .values()
            .filter(|action| action.action.is_sync())
            .max_by_key(|action| action.at_ms)
            .cloned()
    }

    /// Kinds of action whose newest entry on `catalog_id` was refused or
    /// failed, newest first, while nothing that does the same work has
    /// worked since: see [`clears`].
    pub fn problems(&self, catalog_id: &str) -> Vec<LatestAction> {
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(kinds) = latest.by_catalog.get(catalog_id) else {
            return Vec::new();
        };
        let worked_since = |problem: &LatestAction| {
            kinds.values().any(|other| {
                other.outcome == AuditOutcome::Ok
                    // Strictly later: entries read back from the log keep
                    // whole seconds, and a tie proves nothing.
                    && other.at_ms > problem.at_ms
                    && clears(other, problem)
            })
        };
        let mut problems: Vec<_> = kinds
            .values()
            .filter(|action| action.outcome != AuditOutcome::Ok && !worked_since(action))
            .cloned()
            .collect();
        problems.sort_by_key(|action| std::cmp::Reverse(action.at_ms));
        problems
    }

    /// When a Director plugin last uploaded to `catalog_id`, whatever the
    /// outcome: any authenticated upload is contact.
    pub fn director_upload_at(&self, catalog_id: &str) -> Option<i64> {
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        latest
            .by_catalog
            .get(catalog_id)?
            .values()
            .filter(|action| action.action == AuditAction::DirectorImageUpload)
            .map(|action| action.at_ms)
            .max()
    }

    /// The newest notable actions on `catalog_id`, newest first, at most
    /// `limit`, from the end of the log and its previous generation. Reads
    /// files, so call it off the async runtime.
    pub fn history(&self, catalog_id: &str, limit: usize) -> Vec<LatestAction> {
        let mut entries = Vec::new();
        // The previous generation follows on only when the current file is
        // read whole; otherwise the list would jump over the unread middle.
        let current_whole =
            fs::metadata(&self.path).map_or(true, |meta| meta.len() <= HISTORY_TAIL_BYTES);
        let previous = current_whole.then(|| self.path.with_extension("jsonl.1"));
        // Skip other catalogs' lines before parsing them.
        let marker = format!("\"catalog_id\":{}", serde_json::Value::from(catalog_id));
        for path in previous.into_iter().chain([self.path.clone()]) {
            let Ok(tail) = read_tail(&path, HISTORY_TAIL_BYTES) else {
                continue;
            };
            entries.extend(
                tail.lines()
                    .filter(|line| line.contains(&marker))
                    .filter_map(|line| {
                        let entry = serde_json::from_str::<LoggedEntry>(line).ok()?;
                        if entry.catalog_id != catalog_id
                            || !is_notable(entry.action, entry.detail.as_deref())
                        {
                            return None;
                        }
                        let at = chrono::DateTime::parse_from_rfc3339(&entry.at).ok()?;
                        Some(LatestAction {
                            at_ms: at.timestamp_millis(),
                            action: entry.action,
                            outcome: entry.outcome,
                            operation: entry.operation,
                            detail: entry.detail,
                            summary: entry.summary,
                        })
                    }),
            );
        }
        entries.reverse();
        entries.truncate(limit);
        entries
    }

    fn remember(&self, catalog_id: &str, action: LatestAction) {
        let (generation, snapshot) = {
            let mut latest = self
                .latest
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            latest
                .by_catalog
                .entry(catalog_id.to_owned())
                .or_default()
                .insert(kind_of(action.action, action.operation.as_deref()), action);
            latest.generation += 1;
            (latest.generation, latest.by_catalog.clone())
        };
        let path = self.latest_path.clone();
        let written = self.latest_written.clone();
        let write = move || {
            let Ok(mut last) = written.lock() else {
                return;
            };
            if *last >= generation {
                return;
            }
            match write_json_atomically(&path, &snapshot) {
                Ok(()) => *last = generation,
                Err(error) => tracing::debug!(%error, "latest Sync actions were not saved"),
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(runtime) => {
                runtime.spawn_blocking(write);
            }
            Err(_) => write(),
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

/// Whether `worked`, a success, shows that `problem` no longer applies. An
/// upload clears an upload from the same plugin. A sync exchange clears one with the same
/// operation (a merge preview refused while scheduler sync was off clears
/// once a merge works), and one without an operation clears with the same
/// action. Uploads never clear a sync exchange, so a stream of frames does
/// not hide a refused merge, and a preview or export, which reads only,
/// never clears a refused apply.
fn clears(worked: &LatestAction, problem: &LatestAction) -> bool {
    if problem.action.is_upload() || worked.action.is_upload() {
        // Each plugin's uploads clear only its own.
        return problem.action == worked.action;
    }
    let reads_only = |action: AuditAction| {
        matches!(
            action,
            AuditAction::Preview | AuditAction::PreviewRefresh | AuditAction::Export
        )
    };
    if reads_only(worked.action) && !reads_only(problem.action) {
        return false;
    }
    match &problem.operation {
        Some(operation) => worked.operation.as_ref() == Some(operation),
        None => worked.action == problem.action,
    }
}

/// Whether the Live page shows this entry as an action. A capabilities call,
/// a flat-history poll and a sequence report count only as contact, and so
/// does a star-detection snapshot the server already had.
fn is_notable(action: AuditAction, detail: Option<&str>) -> bool {
    action.is_notable()
        && !(action == AuditAction::StarDetectionProfile && detail == Some("unchanged"))
}

fn kind_of(action: AuditAction, operation: Option<&str>) -> String {
    let action = serde_json::to_value(action)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default();
    match operation {
        // An upload's operation is the frame kind; each plugin's uploads
        // are one kind.
        Some(operation) if !action.ends_with("image_upload") => format!("{action}:{operation}"),
        _ => action,
    }
}

/// The newest notable entry of each kind per catalog, from the end of the
/// log and, when the log rolled recently, the end of the previous generation.
fn read_latest_from_log(path: &Path) -> LatestByKind {
    let mut latest = LatestByKind::new();
    for path in [path.with_extension("jsonl.1"), path.to_path_buf()] {
        let Ok(tail) = read_tail(&path, LATEST_TAIL_BYTES) else {
            continue;
        };
        for line in tail.lines() {
            let Ok(entry) = serde_json::from_str::<LoggedEntry>(line) else {
                continue;
            };
            if !is_notable(entry.action, entry.detail.as_deref()) {
                continue;
            }
            let Ok(at) = chrono::DateTime::parse_from_rfc3339(&entry.at) else {
                continue;
            };
            let kind = kind_of(entry.action, entry.operation.as_deref());
            latest.entry(entry.catalog_id).or_default().insert(
                kind,
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

fn write_json_atomically(path: &Path, value: &impl Serialize) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, serde_json::to_vec(value)?)?;
    fs::rename(&temporary, path)?;
    Ok(())
}

/// The last `limit` bytes of `path`, from the first whole line.
fn read_tail(path: &Path, limit: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    // Start one byte early: when that byte ends a line, the next one is whole.
    let start = length.saturating_sub(limit.saturating_add(1));
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    Ok(if start == 0 {
        text.into_owned()
    } else {
        // Drop everything up to the first line break: a cut line, or only
        // the break before a whole line.
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
    fn without_a_saved_record_the_end_of_the_log_is_read_back() {
        let directory = tempdir().unwrap();
        let path = RemoteAuditLog::path_in(directory.path());
        let line = r#"{"at":"2026-11-08T10:00:00Z","catalog_id":"rig","action":"image_upload","outcome":"ok","detail":"M31_0001.fits"}"#;
        // The tail starts exactly at a line: that line is whole and kept.
        let filler = format!("{}\n", "x".repeat(LATEST_TAIL_BYTES as usize));
        fs::write(&path, format!("{filler}not json\n{line}\n")).unwrap();
        let latest = RemoteAuditLog::new(directory.path()).latest("rig").unwrap();
        assert_eq!(latest.action, AuditAction::ImageUpload);
        assert_eq!(latest.detail.as_deref(), Some("M31_0001.fits"));
        assert_eq!(latest.at_ms, 1_794_132_000_000);

        let exact = format!("{line}\n");
        let padding = LATEST_TAIL_BYTES as usize - exact.len();
        fs::write(&path, format!("{}\n{exact}", "y".repeat(padding))).unwrap();
        let tail = read_tail(&path, exact.len() as u64).unwrap();
        assert_eq!(tail, exact);
    }

    #[test]
    fn the_saved_record_outlives_a_log_full_of_polls() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        log.record(
            "rig",
            AuditAction::Apply,
            AuditOutcome::Ok,
            AuditRecord {
                operation: Some("merge"),
                ..Default::default()
            },
        );
        // Polls push the merge out of the end of the log.
        fs::write(log.path(), "").unwrap();
        for _ in 0..50 {
            log.record(
                "rig",
                AuditAction::FlatHistoryPending,
                AuditOutcome::Ok,
                AuditRecord::default(),
            );
        }
        let reopened = RemoteAuditLog::new(directory.path());
        assert_eq!(
            reopened.latest("rig").and_then(|latest| latest.operation),
            Some("merge".into())
        );
    }

    #[test]
    fn a_problem_stays_until_the_same_work_succeeds() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        let tick = || std::thread::sleep(std::time::Duration::from_millis(2));
        let record = |action, outcome, operation: Option<&str>, detail: Option<&str>| {
            tick();
            log.record(
                "rig",
                action,
                outcome,
                AuditRecord {
                    operation,
                    detail,
                    ..Default::default()
                },
            );
        };
        // A merge preview refused while scheduler sync was off.
        record(
            AuditAction::Preview,
            AuditOutcome::Refused,
            Some("merge"),
            Some("remote scheduler sync is disabled for this database"),
        );
        record(
            AuditAction::ImageUpload,
            AuditOutcome::Ok,
            Some("light"),
            Some("M31_0001.fits"),
        );
        record(
            AuditAction::StarDetectionProfile,
            AuditOutcome::Ok,
            Some("star_detection_profile"),
            Some("unchanged"),
        );
        record(
            AuditAction::Apply,
            AuditOutcome::Ok,
            Some("push_grades"),
            None,
        );
        // Uploads, an unchanged snapshot and another operation leave it.
        assert_eq!(log.latest("rig").unwrap().action, AuditAction::Apply);
        assert_eq!(log.problems("rig").len(), 1);
        // A merge that works clears it.
        record(AuditAction::Apply, AuditOutcome::Ok, Some("merge"), None);
        assert!(log.problems("rig").is_empty());

        // A refused apply stays while each cycle's preview works.
        record(
            AuditAction::Apply,
            AuditOutcome::Refused,
            Some("merge"),
            Some("destination changed"),
        );
        record(AuditAction::Preview, AuditOutcome::Ok, Some("merge"), None);
        record(AuditAction::Export, AuditOutcome::Ok, Some("merge"), None);
        assert_eq!(
            log.problems("rig")[0].detail.as_deref(),
            Some("destination changed")
        );
        record(AuditAction::Apply, AuditOutcome::Ok, Some("merge"), None);
        assert!(log.problems("rig").is_empty());

        // An upload clears an upload, and only an upload.
        record(
            AuditAction::ImageUpload,
            AuditOutcome::Refused,
            None,
            Some("sha256 mismatch"),
        );
        record(AuditAction::Apply, AuditOutcome::Ok, Some("merge"), None);
        assert_eq!(log.problems("rig").len(), 1);
        record(
            AuditAction::ImageUpload,
            AuditOutcome::Ok,
            Some("light"),
            Some("M31_0002.fits"),
        );
        assert!(log.problems("rig").is_empty());
    }

    #[test]
    fn a_success_in_the_same_second_does_not_clear_a_problem() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        let entry = |at_ms, action, outcome| LatestAction {
            at_ms,
            action,
            outcome,
            operation: Some("merge".into()),
            detail: None,
            summary: BTreeMap::new(),
        };
        // Read back from the log, both carry whole seconds.
        log.remember(
            "rig",
            entry(1_000, AuditAction::Apply, AuditOutcome::Refused),
        );
        log.remember("rig", entry(1_000, AuditAction::Preview, AuditOutcome::Ok));
        assert_eq!(log.problems("rig").len(), 1);
    }

    #[test]
    fn the_history_lists_one_database_newest_first() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        for operation in ["merge", "push_planning", "push_grades"] {
            log.record(
                "rig",
                AuditAction::Apply,
                AuditOutcome::Ok,
                AuditRecord {
                    operation: Some(operation),
                    ..Default::default()
                },
            );
            log.record(
                "rig",
                AuditAction::Capabilities,
                AuditOutcome::Ok,
                AuditRecord::default(),
            );
            log.record(
                "other",
                AuditAction::Export,
                AuditOutcome::Ok,
                AuditRecord::default(),
            );
        }
        let history = log.history("rig", 2);
        let operations: Vec<_> = history
            .iter()
            .map(|entry| entry.operation.as_deref())
            .collect();
        assert_eq!(operations, [Some("push_grades"), Some("push_planning")]);
        assert!(log.history("missing", 10).is_empty());
    }

    #[test]
    fn refused_sequence_reports_are_logged_once_in_a_while() {
        let directory = tempdir().unwrap();
        let log = RemoteAuditLog::new(directory.path());
        for _ in 0..5 {
            log.record(
                "rig",
                AuditAction::SequenceStatus,
                AuditOutcome::Refused,
                AuditRecord {
                    detail: Some("remote scheduler sync is disabled"),
                    ..Default::default()
                },
            );
        }
        log.record(
            "other",
            AuditAction::SequenceStatus,
            AuditOutcome::Refused,
            AuditRecord::default(),
        );
        let contents = fs::read_to_string(log.path()).unwrap();
        assert_eq!(contents.lines().count(), 2, "{contents}");
        assert!(log.latest("rig").is_none());
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
