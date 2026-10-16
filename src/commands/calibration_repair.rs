//! `psf-guard calibration repair-duplicates`: find calibration rows that
//! record one exposure more than once, and merge them. See
//! [`crate::calibration::plan_duplicate_repair`].

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::calibration::{self, CheckStage, Checking, DuplicateReport, FileAccess};

pub fn repair_duplicates(db: &Path, dry_run: bool, map_prefix: Option<&str>) -> Result<()> {
    let files = match map_prefix {
        Some(mapping) => {
            let Some((from, to)) = mapping.split_once('=') else {
                bail!("--map-prefix takes FROM=TO, not {mapping:?}");
            };
            FileAccess::remapped(PathBuf::from(from), PathBuf::from(to))
        }
        None => FileAccess::disk(),
    };
    let flags = if dry_run {
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX
    } else {
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX
    };
    // A repair holds the catalog's repair lock from its check to its
    // commit, so a server cannot repair the catalog meanwhile, nor this
    // run repair one a server is repairing.
    let _lock =
        if dry_run {
            None
        } else {
            Some(calibration::repair_lock(db).with_context(|| {
                format!("locking {}", calibration::repair_lock_path(db).display())
            })?)
        };
    let mut conn = Connection::open_with_flags(db, flags)
        .with_context(|| format!("opening {}", db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(30))?;
    let snapshot = calibration::duplicate_snapshot(&conn)?;
    let began = std::time::Instant::now();
    let workers = crate::concurrency::plan_workers(
        None,
        &crate::concurrency::WorkerPolicy::default(),
        crate::concurrency::Priority::Interactive,
        None,
    )
    .workers;
    let stop = AtomicBool::new(false);
    let last = Mutex::new(Instant::now());
    let progress = |stage: CheckStage, done: usize, total: usize| {
        let mut last = last.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if done == total || last.elapsed() >= Duration::from_secs(2) {
            *last = Instant::now();
            let what = match stage {
                CheckStage::Files => "Checking files",
                CheckStage::Copies => "Comparing copies",
            };
            eprintln!("{what} {done} of {total}");
        }
    };
    let report = calibration::plan_duplicate_repair_checked(
        &snapshot,
        &files,
        &Checking {
            workers,
            progress: &progress,
            stop: &stop,
        },
    )
    .map_err(|_| anyhow::anyhow!("the check was stopped"))?;
    eprintln!(
        "Checked the files in {:.2}s on {workers} thread(s).",
        began.elapsed().as_secs_f64()
    );
    print_report(&report);
    if report.is_empty() {
        println!("Nothing to repair.");
        return Ok(());
    }
    if dry_run {
        println!("Dry run: nothing was changed.");
        return Ok(());
    }
    let applying = Instant::now();
    let outcome = calibration::apply_duplicate_repair(&mut conn, &report)?;
    eprintln!("Applied in {:.2}s.", applying.elapsed().as_secs_f64());
    println!(
        "Repaired {} exposure(s): {} row(s) merged, {} moved to a live copy, {} master(s) to rebuild. No file was moved or deleted.",
        outcome.groups, outcome.rows_removed, outcome.repointed, outcome.masters_removed
    );
    println!(
        "A server open on this catalog sees the change at its next scan, regrade or restart; a master it was building from a merged row is not recorded."
    );
    Ok(())
}

fn print_report(report: &DuplicateReport) {
    println!(
        "{} exposure(s) recorded more than once: {} row(s) to merge ({} whose file is gone, {} second copies on disk), {} kept row(s) to move to a live copy, {} master(s) to rebuild.",
        report.groups.len(),
        report.rows_removed,
        report.missing_rows,
        report.physical_rows,
        report.repointed,
        report.masters.len()
    );
    let doubled = report
        .masters
        .iter()
        .filter(|master| master.doubled)
        .count();
    if doubled > 0 {
        println!("  {doubled} of those masters averaged an exposure twice.");
    }
    let mut by_kind = std::collections::BTreeMap::<&str, (usize, usize)>::new();
    for group in &report.groups {
        let entry = by_kind.entry(group.kind.as_str()).or_default();
        entry.0 += group.members.iter().filter(|member| !member.exists).count();
        entry.1 += group
            .members
            .iter()
            .filter(|member| member.exists)
            .count()
            .saturating_sub(1);
    }
    for (kind, (missing, physical)) in by_kind {
        println!("  {kind:<9} {missing:>5} missing  {physical:>5} second copies");
    }
    for group in &report.groups {
        println!(
            "\n{} {} ({})",
            group.kind.as_str(),
            group.file_name,
            group.rig_name.as_deref().unwrap_or(&group.rig_uuid)
        );
        for member in &group.members {
            let role = if member.keep { "keep " } else { "merge" };
            let state = if member.exists { "on disk" } else { "missing" };
            println!(
                "  {role} {} {state:<7} {}",
                member.frame_uuid, member.source_path
            );
        }
        if group.repointed {
            println!("  kept row moves to {}", group.path);
        }
    }
    if !report.masters.is_empty() {
        println!("\nMasters to rebuild:");
        for master in &report.masters {
            println!(
                "  {} {:<5} {} frame(s){}",
                master.master_uuid,
                master.kind,
                master.source_count,
                if master.doubled {
                    ", averaged an exposure twice"
                } else if master.direct {
                    ""
                } else {
                    ", built on one of the above"
                }
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_line_waits_its_turn_behind_a_server_repair() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = dir.path().join("catalog.sqlite");
        let conn = Connection::open(&catalog).unwrap();
        calibration::ensure_schema(&conn).unwrap();
        drop(conn);
        // A server is repairing this catalog.
        let server = calibration::repair_lock(&catalog).unwrap();
        let refused = repair_duplicates(&catalog, false, None).unwrap_err();
        assert!(
            format!("{refused:#}").contains("another repair of this catalog is running"),
            "{refused:#}"
        );
        // A dry run writes nothing, so it needs no lock.
        repair_duplicates(&catalog, true, None).unwrap();
        drop(server);
        repair_duplicates(&catalog, false, None).unwrap();
    }
}
