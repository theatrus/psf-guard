//! One-way grading-state push (our DB → telescope).
//!
//! Pushes grading state (`gradingStatus` + `rejectreason`) from a source Target
//! Scheduler database into a destination database. Images are matched by the
//! stable `acquiredimage.guid` (TS plugin schema v22+), so the two databases
//! are assumed same-lineage (one a copy/export of the other). The source always
//! wins; running it in both directions gives a crude bidirectional reconcile.
//!
//! Pure DB logic: the CLI glue (argument resolution, connection opening,
//! reporting) lives in `cli_main.rs`; shared helpers live in the module root.

use crate::commands::reject_archive::require_target_scheduler_guid;
use crate::db::Database;
use crate::models::GradingStatus;
use anyhow::{anyhow, Context, Result};
use rusqlite::Connection;
use std::collections::{BTreeMap, HashMap, HashSet};

/// Knobs for a one-way grade push.
pub struct SyncGradesOptions {
    /// Only consider source rows with this grade (None = all).
    pub status_filter: Option<GradingStatus>,
    /// Restrict to source rows whose project name matches (substring).
    pub project_filter: Option<String>,
    /// Restrict to source rows whose target name matches (substring).
    pub target_filter: Option<String>,
    /// Compute the plan but make no writes to the destination.
    pub dry_run: bool,
}

/// A single staged grade change — a destination guid moving from one grade to
/// another. Returned (rather than printed) so the CLI/UI layer can render a
/// trace while the core sync stays side-effect free.
#[derive(Debug, Clone)]
pub struct GradeChange {
    pub guid: String,
    pub from: i32,
    pub to: i32,
    pub reason: Option<String>,
}

/// Outcome counters for a grade push.
#[derive(Debug, Default)]
pub struct SyncSummary {
    /// Source rows (after filters) that had a usable, non-duplicate guid.
    pub source_considered: usize,
    /// Source rows skipped because their guid was NULL/empty.
    pub source_no_guid: usize,
    /// Considered source rows whose guid matched a destination row.
    pub matched: usize,
    /// Matched rows where grade + reason already agreed.
    pub unchanged: usize,
    /// Matched rows updated in the destination.
    pub changed: usize,
    /// Considered source rows whose guid was absent from the destination.
    pub unmatched_source: usize,
    /// Destination guids never matched by a considered source row.
    pub dest_only: usize,
    /// Distinct guids skipped because they appeared more than once in either DB.
    pub duplicate_guids: usize,
    /// Per-transition counts, e.g. `"Pending→Rejected" => 12`.
    pub transitions: BTreeMap<String, usize>,
    /// Each staged change, in source iteration order (for verbose tracing).
    pub changes: Vec<GradeChange>,
}

fn grade_from_i32(v: i32) -> Result<GradingStatus> {
    match v {
        0 => Ok(GradingStatus::Pending),
        1 => Ok(GradingStatus::Accepted),
        2 => Ok(GradingStatus::Rejected),
        other => Err(anyhow!(
            "Unexpected gradingStatus value {} in source database",
            other
        )),
    }
}

/// Push grading state from `src` into `dest`, matching by `acquiredimage.guid`.
///
/// Both databases must have the `guid` column (TS schema v22+); otherwise this
/// returns an error naming the offending side. The destination is written in a
/// single transaction (unless `dry_run`).
pub fn sync_grades(
    src: &Connection,
    dest: &Connection,
    opts: &SyncGradesOptions,
) -> Result<SyncSummary> {
    require_target_scheduler_guid(src).context("source database")?;
    require_target_scheduler_guid(dest).context("destination database")?;

    let src_db = Database::new(src);
    let dest_db = Database::new(dest);

    let src_rows = src_db.query_images(
        opts.status_filter,
        opts.project_filter.as_deref(),
        opts.target_filter.as_deref(),
        None,
    )?;
    let dest_rows = dest_db.query_images(None, None, None, None)?;

    let mut summary = SyncSummary::default();

    // Build destination guid -> (id, grade, reason); drop any guid that occurs
    // more than once (ambiguous — guids are meant to be unique).
    let mut dest_map: HashMap<String, (i32, i32, Option<String>)> = HashMap::new();
    let mut dest_dups: HashSet<String> = HashSet::new();
    for (img, _, _) in &dest_rows {
        match img.guid.as_deref() {
            Some(g) if !g.is_empty() => {
                if dest_dups.contains(g) {
                    continue;
                }
                if dest_map.remove(g).is_some() {
                    dest_dups.insert(g.to_string());
                } else {
                    dest_map.insert(
                        g.to_string(),
                        (img.id, img.grading_status, img.reject_reason.clone()),
                    );
                }
            }
            _ => {}
        }
    }

    // Count guid occurrences in the (filtered) source to spot duplicates.
    let mut src_counts: HashMap<&str, usize> = HashMap::new();
    for (img, _, _) in &src_rows {
        if let Some(g) = img.guid.as_deref() {
            if !g.is_empty() {
                *src_counts.entry(g).or_insert(0) += 1;
            }
        }
    }

    let mut matched_dest: HashSet<&str> = HashSet::new();
    let mut src_dup_guids: HashSet<&str> = HashSet::new();
    let mut updates: Vec<(i32, GradingStatus, Option<String>)> = Vec::new();

    for (img, _, _) in &src_rows {
        let guid = match img.guid.as_deref() {
            Some(g) if !g.is_empty() => g,
            _ => {
                summary.source_no_guid += 1;
                continue;
            }
        };
        if src_counts.get(guid).copied().unwrap_or(0) > 1 {
            src_dup_guids.insert(guid);
            continue;
        }
        summary.source_considered += 1;

        if dest_dups.contains(guid) {
            // Ambiguous on the destination side; leave it alone.
            continue;
        }
        let (dest_id, dest_grade, dest_reason) = match dest_map.get(guid) {
            Some(entry) => entry,
            None => {
                summary.unmatched_source += 1;
                continue;
            }
        };
        matched_dest.insert(guid);
        summary.matched += 1;

        if img.grading_status == *dest_grade && img.reject_reason == *dest_reason {
            summary.unchanged += 1;
            continue;
        }

        let status = grade_from_i32(img.grading_status)?;
        updates.push((*dest_id, status, img.reject_reason.clone()));
        summary.changed += 1;
        let label = format!(
            "{}→{}",
            GradingStatus::from_i32(*dest_grade),
            GradingStatus::from_i32(img.grading_status),
        );
        *summary.transitions.entry(label).or_insert(0) += 1;
        summary.changes.push(GradeChange {
            guid: guid.to_string(),
            from: *dest_grade,
            to: img.grading_status,
            reason: img.reject_reason.clone(),
        });
    }

    // Distinct guids that were ambiguous in either DB (a guid duplicated in
    // *both* must only be counted once).
    let mut dup_guids: HashSet<&str> = src_dup_guids.clone();
    dup_guids.extend(dest_dups.iter().map(|g| g.as_str()));
    summary.duplicate_guids = dup_guids.len();
    summary.dest_only = dest_map
        .keys()
        .filter(|g| !matched_dest.contains(g.as_str()))
        .count();

    if !opts.dry_run && !updates.is_empty() {
        dest_db
            .batch_update_grading_status(&updates)
            .context("applying grade updates to destination")?;
    }

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;

    /// `images`: (Id, gradingStatus, guid, rejectreason).
    fn setup_db(images: &[(i32, i32, Option<&str>, Option<&str>)], with_guid: bool) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE project (Id INTEGER PRIMARY KEY, profileId TEXT, name TEXT, description TEXT, guid TEXT);
             CREATE TABLE target (Id INTEGER PRIMARY KEY, name TEXT, active INTEGER, ra REAL, dec REAL, projectId INTEGER, guid TEXT);
             INSERT INTO project (Id, profileId, name, description, guid) VALUES (1, 'p', 'Proj', '', 'pg');
             INSERT INTO target (Id, name, active, ra, dec, projectId, guid) VALUES (1, 'Tgt', 1, 0, 0, 1, 'tg');",
        )
        .unwrap();
        if with_guid {
            conn.execute_batch(
                "CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER, metadata TEXT, rejectreason TEXT, profileId TEXT, guid TEXT);",
            )
            .unwrap();
        } else {
            conn.execute_batch(
                "CREATE TABLE acquiredimage (Id INTEGER PRIMARY KEY, projectId INTEGER, targetId INTEGER, acquireddate INTEGER, filtername TEXT, gradingStatus INTEGER, metadata TEXT, rejectreason TEXT, profileId TEXT);",
            )
            .unwrap();
        }
        for (id, grade, guid, reason) in images {
            if with_guid {
                conn.execute(
                    "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, rejectreason, profileId, guid)
                     VALUES (?, 1, 1, 0, 'L', ?, '{}', ?, 'p', ?)",
                    params![id, grade, reason, guid],
                )
                .unwrap();
            } else {
                conn.execute(
                    "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername, gradingStatus, metadata, rejectreason, profileId)
                     VALUES (?, 1, 1, 0, 'L', ?, '{}', ?, 'p')",
                    params![id, grade, reason],
                )
                .unwrap();
            }
        }
        conn
    }

    fn opts() -> SyncGradesOptions {
        SyncGradesOptions {
            status_filter: None,
            project_filter: None,
            target_filter: None,
            dry_run: false,
        }
    }

    fn grade_of(conn: &Connection, guid: &str) -> (i32, Option<String>) {
        conn.query_row(
            "SELECT gradingStatus, rejectreason FROM acquiredimage WHERE guid = ?",
            params![guid],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap()
    }

    #[test]
    fn pushes_changes_leaves_agreements() {
        // Destination uses different Ids on purpose: matching is by guid, not Id.
        let src = setup_db(
            &[
                (1, 1, Some("g1"), None),           // Accepted
                (2, 2, Some("g2"), Some("clouds")), // Rejected w/ reason
                (3, 0, Some("g3"), None),           // Pending
            ],
            true,
        );
        let dest = setup_db(
            &[
                (100, 0, Some("g1"), None),
                (200, 0, Some("g2"), None),
                (300, 0, Some("g3"), None),
            ],
            true,
        );

        let summary = sync_grades(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.source_considered, 3);
        assert_eq!(summary.matched, 3);
        assert_eq!(summary.changed, 2); // g1, g2
        assert_eq!(summary.unchanged, 1); // g3 already Pending
        assert_eq!(summary.unmatched_source, 0);
        assert_eq!(summary.dest_only, 0);

        assert_eq!(grade_of(&dest, "g1"), (1, None));
        assert_eq!(grade_of(&dest, "g2"), (2, Some("clouds".to_string())));
        assert_eq!(grade_of(&dest, "g3"), (0, None));
    }

    #[test]
    fn dry_run_makes_no_writes() {
        let src = setup_db(&[(1, 2, Some("g1"), Some("bad"))], true);
        let dest = setup_db(&[(100, 0, Some("g1"), None)], true);

        let mut o = opts();
        o.dry_run = true;
        let summary = sync_grades(&src, &dest, &o).unwrap();
        assert_eq!(summary.changed, 1);
        // Destination untouched.
        assert_eq!(grade_of(&dest, "g1"), (0, None));
    }

    #[test]
    fn unmatched_and_dest_only_counted() {
        let src = setup_db(&[(1, 1, Some("g1"), None), (2, 1, Some("g2"), None)], true);
        let dest = setup_db(
            &[(10, 0, Some("g1"), None), (20, 0, Some("g3"), None)],
            true,
        );

        let summary = sync_grades(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.matched, 1); // g1
        assert_eq!(summary.changed, 1); // g1 0->1
        assert_eq!(summary.unmatched_source, 1); // g2 not in dest
        assert_eq!(summary.dest_only, 1); // g3 not in source
        assert_eq!(grade_of(&dest, "g1"), (1, None));
        assert_eq!(grade_of(&dest, "g3"), (0, None)); // untouched
    }

    #[test]
    fn status_filter_scopes_source() {
        let src = setup_db(
            &[(1, 2, Some("g1"), Some("x")), (2, 1, Some("g2"), None)],
            true,
        );
        let dest = setup_db(
            &[(10, 0, Some("g1"), None), (20, 0, Some("g2"), None)],
            true,
        );

        let mut o = opts();
        o.status_filter = Some(GradingStatus::Rejected);
        let summary = sync_grades(&src, &dest, &o).unwrap();
        assert_eq!(summary.source_considered, 1); // only g1 (Rejected)
        assert_eq!(summary.changed, 1);
        assert_eq!(grade_of(&dest, "g1"), (2, Some("x".to_string())));
        assert_eq!(grade_of(&dest, "g2"), (0, None)); // not pushed (filtered out)
    }

    #[test]
    fn rows_without_guid_are_skipped() {
        let src = setup_db(&[(1, 1, Some("g1"), None), (2, 1, None, None)], true);
        let dest = setup_db(&[(10, 0, Some("g1"), None)], true);

        let summary = sync_grades(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.source_no_guid, 1);
        assert_eq!(summary.source_considered, 1);
        assert_eq!(summary.changed, 1);
    }

    #[test]
    fn missing_guid_column_is_refused() {
        let src = setup_db(&[(1, 1, Some("g1"), None)], true);
        let dest = setup_db(&[(10, 0, None, None)], false); // no guid column
        let err = sync_grades(&src, &dest, &opts()).unwrap_err();
        assert!(format!("{:#}", err).contains("destination"));
    }

    #[test]
    fn duplicate_dest_guid_is_skipped() {
        let src = setup_db(&[(1, 2, Some("g1"), Some("x"))], true);
        // g1 appears twice in destination -> ambiguous, must not be written.
        let dest = setup_db(
            &[(10, 0, Some("g1"), None), (11, 0, Some("g1"), None)],
            true,
        );

        let summary = sync_grades(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.duplicate_guids, 1);
        assert_eq!(summary.changed, 0);
        // Both copies still Pending.
        let count: i32 = dest
            .query_row(
                "SELECT COUNT(*) FROM acquiredimage WHERE guid='g1' AND gradingStatus=0",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn duplicate_guid_in_both_dbs_counts_once() {
        // g1 is duplicated in BOTH source and destination. It must contribute
        // a single distinct entry to duplicate_guids, not two.
        let src = setup_db(
            &[(1, 2, Some("g1"), Some("x")), (2, 2, Some("g1"), Some("y"))],
            true,
        );
        let dest = setup_db(
            &[(10, 0, Some("g1"), None), (11, 0, Some("g1"), None)],
            true,
        );

        let summary = sync_grades(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.duplicate_guids, 1);
        assert_eq!(summary.changed, 0);
    }

    #[test]
    fn changes_trace_is_returned_not_printed() {
        let src = setup_db(&[(1, 2, Some("g1"), Some("clouds"))], true);
        let dest = setup_db(&[(10, 0, Some("g1"), None)], true);

        let summary = sync_grades(&src, &dest, &opts()).unwrap();
        assert_eq!(summary.changes.len(), 1);
        let c = &summary.changes[0];
        assert_eq!(c.guid, "g1");
        assert_eq!((c.from, c.to), (0, 2));
        assert_eq!(c.reason.as_deref(), Some("clouds"));
    }
}
