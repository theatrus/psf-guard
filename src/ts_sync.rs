//! Synchronize a PSF Guard-owned catalog with its external Target Scheduler file.
//!
//! Capture imports retain the existing GUID merge rules. Automatic writeback
//! sends only local grade changes, against the last observed TS grade. The
//! pending table makes a TS commit followed by a host crash safe to retry.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source_path: String,
    pub writeback: bool,
    pub on_open: bool,
    pub on_change: bool,
    /// Zero disables the schedule; change detection and on-open remain separate.
    pub interval_minutes: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source_path: String::new(),
            writeback: true,
            on_open: true,
            on_change: true,
            interval_minutes: 5,
        }
    }
}

impl Settings {
    pub fn validate(&self, catalog: &std::path::Path) -> Result<()> {
        anyhow::ensure!(
            self.interval_minutes <= 7 * 24 * 60,
            "Choose a TS sync interval from 0 to 10080 minutes. Zero disables scheduled sync."
        );
        let source = std::path::Path::new(&self.source_path);
        anyhow::ensure!(
            source.is_absolute(),
            "Choose an absolute Target Scheduler database path."
        );
        let same = source == catalog
            || std::fs::canonicalize(source)
                .ok()
                .zip(std::fs::canonicalize(catalog).ok())
                .is_some_and(|(source, catalog)| source == catalog);
        anyhow::ensure!(
            !same,
            "The TS sync file must be different from the PSF Guard catalog."
        );
        Ok(())
    }
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS psf_guard_ts_sync_grade (
    guid TEXT PRIMARY KEY NOT NULL,
    local_grade INTEGER NOT NULL, local_reason TEXT,
    source_grade INTEGER NOT NULL, source_reason TEXT
);
CREATE TABLE IF NOT EXISTS psf_guard_ts_sync_pending (
    guid TEXT PRIMARY KEY NOT NULL, grade INTEGER NOT NULL, reason TEXT
);";

#[derive(Debug, Clone, PartialEq, Eq)]
struct Grade {
    status: i32,
    reason: Option<String>,
}

fn grades(conn: &Connection) -> Result<HashMap<String, Grade>> {
    let mut statement = conn.prepare("SELECT guid, gradingStatus, rejectreason FROM acquiredimage WHERE guid IS NOT NULL AND trim(guid) <> ''")?;
    let rows = statement.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            Grade {
                status: r.get(1)?,
                reason: r.get(2)?,
            },
        ))
    })?;
    let mut result = HashMap::new();
    let mut duplicates = HashSet::new();
    for row in rows {
        let (guid, grade) = row?;
        if result.insert(guid.clone(), grade).is_some() {
            duplicates.insert(guid);
        }
    }
    for guid in duplicates {
        result.remove(&guid);
    }
    Ok(result)
}

fn baseline(conn: &Connection, guid: &str, local: &Grade, source: &Grade) -> Result<()> {
    conn.execute("INSERT INTO psf_guard_ts_sync_grade VALUES(?1,?2,?3,?4,?5)
        ON CONFLICT(guid) DO UPDATE SET local_grade=excluded.local_grade,
        local_reason=excluded.local_reason,source_grade=excluded.source_grade,source_reason=excluded.source_reason
        WHERE local_grade IS NOT excluded.local_grade OR local_reason IS NOT excluded.local_reason
           OR source_grade IS NOT excluded.source_grade OR source_reason IS NOT excluded.source_reason",
        params![guid,local.status,local.reason,source.status,source.reason])?;
    Ok(())
}

/// Call on the owned snapshot only, never on an external TS source.
pub fn initialize(conn: &Connection) -> Result<()> {
    crate::commands::sync::require_pull_capable(conn)?;
    conn.execute_batch(SCHEMA)?;
    // A copied catalog may contain a different installation's connection
    // history. Only the new owned copy is reset.
    conn.execute_batch(
        "DELETE FROM psf_guard_ts_sync_pending; DELETE FROM psf_guard_ts_sync_grade;",
    )?;
    for (guid, grade) in grades(conn)? {
        baseline(conn, &guid, &grade, &grade)?;
    }
    Ok(())
}

#[derive(Debug, Default, Clone, Serialize)]
pub struct Report {
    #[serde(skip)]
    pub(crate) arrivals: Vec<crate::commands::sync::ChangedAcquiredImage>,
    pub inserted: usize,
    pub updated: usize,
    pub grades_written: usize,
    pub conflicts: usize,
}

/// `snapshot` is a consistent, read-only copy of TS. `destination` is the
/// live TS writer, supplied only when writeback is enabled. No local write
/// transaction remains open during the external write.
pub fn synchronize(
    local: &mut Connection,
    snapshot: &Connection,
    mut destination: Option<&mut Connection>,
) -> Result<Report> {
    crate::commands::sync::require_pull_capable(snapshot)?;
    local.execute_batch(SCHEMA)?;
    let incoming = grades(snapshot)?;
    let tx = local.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // Recover an external commit whose local acknowledgement was interrupted.
    let pending = {
        let mut q = tx.prepare("SELECT guid,grade,reason FROM psf_guard_ts_sync_pending")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                Grade {
                    status: r.get(1)?,
                    reason: r.get(2)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (guid, desired) in pending {
        if incoming.get(&guid) == Some(&desired) {
            baseline(&tx, &guid, &desired, &desired)?;
            tx.execute(
                "DELETE FROM psf_guard_ts_sync_pending WHERE guid=?1",
                [&guid],
            )?;
        }
    }
    let before = grades(&tx)?;
    let mut changes = Vec::new();
    let mut dirty = HashSet::new();
    let mut report = Report::default();
    for (guid, current) in &before {
        let previous = tx.query_row("SELECT local_grade,local_reason,source_grade,source_reason FROM psf_guard_ts_sync_grade WHERE guid=?1", [guid],
            |r| Ok((Grade {status:r.get(0)?,reason:r.get(1)?}, Grade {status:r.get(2)?,reason:r.get(3)?}))).optional()?;
        let Some((previous_local, previous_source)) = previous else {
            continue;
        };
        if current == &previous_local {
            continue;
        }
        dirty.insert(guid.clone());
        let Some(source) = incoming.get(guid) else {
            continue;
        };
        if source == current {
            baseline(&tx, guid, current, source)?;
            tx.execute(
                "DELETE FROM psf_guard_ts_sync_pending WHERE guid=?1",
                [guid],
            )?;
            dirty.remove(guid);
        } else if source != &previous_source {
            report.conflicts += 1;
        } else if destination.is_some() {
            tx.execute("INSERT INTO psf_guard_ts_sync_pending VALUES(?1,?2,?3) ON CONFLICT(guid) DO UPDATE SET grade=excluded.grade,reason=excluded.reason WHERE grade IS NOT excluded.grade OR reason IS NOT excluded.reason",
                params![guid,current.status,current.reason])?;
            changes.push((guid.clone(), current.clone(), previous_source));
        }
    }
    let summary = crate::commands::sync::sync_arrivals(snapshot, &tx, &dirty)?;
    report.inserted = summary.acquiredimage.inserted;
    report.updated = summary.acquiredimage.updated;
    report.arrivals = summary.changed_acquiredimages;
    let after = grades(&tx)?;
    for (guid, source) in &incoming {
        if !dirty.contains(guid)
            && let Some(current) = after.get(guid)
        {
            // Adoption can replace a PSF Guard-minted GUID with the TS GUID.
            // Preserve a reviewed local decision as a pending local edit.
            let initial = if !before.contains_key(guid) {
                source
            } else {
                current
            };
            baseline(&tx, guid, initial, source)?;
            tx.execute(
                "DELETE FROM psf_guard_ts_sync_pending WHERE guid=?1",
                [guid],
            )?;
        }
    }
    tx.commit()?;
    if !changes.is_empty()
        && let Some(destination) = destination.as_mut()
    {
        let tx = destination
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .context("locking Target Scheduler for grade writeback")?;
        let mut acknowledged = Vec::new();
        for (guid, desired, expected) in changes {
            let changed = tx.execute(
                "UPDATE acquiredimage SET gradingStatus=?1,rejectreason=?2
                WHERE guid=?3 AND gradingStatus=?4 AND rejectreason IS ?5
                AND (SELECT count(*) FROM acquiredimage WHERE guid=?3)=1",
                params![
                    desired.status,
                    desired.reason,
                    guid,
                    expected.status,
                    expected.reason
                ],
            )?;
            if changed == 1 {
                acknowledged.push((guid, desired));
            } else {
                report.conflicts += 1;
            }
        }
        if !acknowledged.is_empty() {
            crate::db::reconcile_accepted_counts(&tx)?;
        }
        tx.commit()?;
        report.grades_written = acknowledged.len();
        let tx = local.transaction_with_behavior(TransactionBehavior::Immediate)?;
        for (guid, grade) in acknowledged {
            baseline(&tx, &guid, &grade, &grade)?;
            tx.execute(
                "DELETE FROM psf_guard_ts_sync_pending WHERE guid=?1",
                [&guid],
            )?;
        }
        tx.commit()?;
    }
    Ok(report)
}
