//! `psf-guard calibration repair-duplicates`: find calibration rows that
//! record one exposure more than once, and merge them. See
//! [`crate::calibration::plan_duplicate_repair`].

use anyhow::{bail, Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

use crate::calibration::{self, DuplicateReport, FileAccess};

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
    let mut conn = Connection::open_with_flags(db, flags)
        .with_context(|| format!("opening {}", db.display()))?;
    let snapshot = calibration::duplicate_snapshot(&conn)?;
    let report = calibration::plan_duplicate_repair(&snapshot, &files);
    print_report(&report);
    if report.is_empty() {
        println!("Nothing to repair.");
        return Ok(());
    }
    if dry_run {
        println!("Dry run: nothing was changed.");
        return Ok(());
    }
    let outcome = calibration::apply_duplicate_repair(&mut conn, &report)?;
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
