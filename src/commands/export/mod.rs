//! Export ("take out") graded lights into a stacking-friendly folder tree.
//!
//! Selects non-rejected acquired images (Accepted always; Pending with
//! `include_pending`), resolves each to a file on disk via the basename
//! directory index, and lays them out WBPP-style:
//!
//! ```text
//! <dest>/<target>/LIGHT/<filter>/<basename>.fits
//! ```
//!
//! Rejected frames are never exported. The layout deliberately reserves
//! sibling trees for calibration frames — `<target>/FLAT/<filter>/`,
//! `<dest>/DARK/<exposure>_G<gain>/`, `<dest>/BIAS/` — via [`FrameKind`]:
//! a future matcher (flathistory-guided flats, header-scanned darks) only
//! has to emit more [`ExportItem`]s; planning, placement, idempotency and
//! the server's archive streaming all work per-item already.
//!
//! Placement is copy (default) or hardlink (`--link`, same-filesystem);
//! existing destination files with matching size are skipped, so re-running
//! an export after a new night only adds the new subs.

use crate::db::Database;
use crate::directory_tree::DirectoryTree;
use crate::models::GradingStatus;
use anyhow::{Context, Result};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a frame is for the stacking pipeline. Only lights are selected
/// today; the calibration kinds define where future matches will land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Light,
    Flat,
    Dark,
    Bias,
}

#[derive(Debug, Clone)]
pub struct ExportItem {
    pub image_id: i32,
    pub kind: FrameKind,
    pub source: PathBuf,
    /// Path below the destination root (also the archive entry name).
    pub relative_dest: PathBuf,
    pub size_bytes: u64,
}

#[derive(Debug, Default)]
pub struct ExportPlan {
    pub items: Vec<ExportItem>,
    /// (image id, basename) rows whose file was not found in any image dir.
    pub missing: Vec<(i32, String)>,
    /// Rows without a FileName in their metadata.
    pub unresolvable: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ExportOptions {
    pub include_pending: bool,
    /// Substring filters, matching the rest of the CLI.
    pub project_filter: Option<String>,
    pub target_filter: Option<String>,
    /// Exact-id filters (used by the server endpoint).
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    /// Restrict to one filter name (exact, case-insensitive).
    pub filter_name: Option<String>,
}

#[derive(Debug, Default, serde::Serialize)]
pub struct ExportSummary {
    pub planned: usize,
    pub copied: usize,
    pub linked: usize,
    pub skipped_existing: usize,
    pub missing: usize,
    pub errors: usize,
    pub bytes: u64,
}

/// Make a name safe as a single path component (target and filter names are
/// free text in the scheduler DB).
pub fn sanitize_component(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

/// Select frames and resolve them to on-disk files. Pure planning — no
/// filesystem writes — shared by the CLI and the server's archive stream.
pub fn plan_export(
    conn: &Connection,
    image_dirs: &[String],
    options: &ExportOptions,
) -> Result<ExportPlan> {
    let db = Database::new(conn);

    let mut rows = db
        .query_images(
            Some(GradingStatus::Accepted),
            options.project_filter.as_deref(),
            options.target_filter.as_deref(),
            None,
        )
        .context("querying accepted images")?;
    if options.include_pending {
        rows.extend(
            db.query_images(
                Some(GradingStatus::Pending),
                options.project_filter.as_deref(),
                options.target_filter.as_deref(),
                None,
            )
            .context("querying pending images")?,
        );
    }

    let dir_paths: Vec<&Path> = image_dirs.iter().map(Path::new).collect();
    let tree = DirectoryTree::build_multiple(&dir_paths).context("indexing image directories")?;

    let mut plan = ExportPlan::default();
    // Guard against two source files mapping onto one destination name
    // (same basename for a target+filter, e.g. after a manual file copy).
    let mut used_dests: HashMap<PathBuf, usize> = HashMap::new();

    for (image, _project_name, target_name) in rows {
        if options.project_id.is_some_and(|id| image.project_id != id)
            || options.target_id.is_some_and(|id| image.target_id != id)
        {
            continue;
        }
        if let Some(wanted) = &options.filter_name
            && !image.filter_name.eq_ignore_ascii_case(wanted)
        {
            continue;
        }
        let Some(basename) = crate::utils::extract_filename(&image.metadata) else {
            plan.unresolvable += 1;
            continue;
        };
        let Some(source) = tree.find_file_first(&basename).cloned() else {
            plan.missing.push((image.id, basename));
            continue;
        };
        let size_bytes = std::fs::metadata(&source).map(|m| m.len()).unwrap_or(0);

        // The basename comes from the row's metadata JSON; sanitize it too so
        // a degenerate FileName (e.g. "..") can never shift the destination
        // or produce a traversal-shaped archive entry name.
        let mut relative_dest = PathBuf::from(sanitize_component(&target_name))
            .join("LIGHT")
            .join(sanitize_component(&image.filter_name))
            .join(sanitize_component(&basename));
        let clashes = used_dests.entry(relative_dest.clone()).or_insert(0);
        *clashes += 1;
        if *clashes > 1 {
            let stem = source
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "frame".into());
            let ext = source
                .extension()
                .map(|e| format!(".{}", e.to_string_lossy()))
                .unwrap_or_default();
            relative_dest = relative_dest.with_file_name(format!("{}.{}{}", stem, *clashes, ext));
        }

        plan.items.push(ExportItem {
            image_id: image.id,
            kind: FrameKind::Light,
            source,
            relative_dest,
            size_bytes,
        });
    }

    // Deterministic order: by destination path.
    plan.items
        .sort_by(|a, b| a.relative_dest.cmp(&b.relative_dest));
    Ok(plan)
}

/// Place the planned files under `dest_root`. `link` uses hardlinks (falling
/// back to copy when the link fails, e.g. across filesystems).
pub fn execute_plan(
    plan: &ExportPlan,
    dest_root: &Path,
    link: bool,
    dry_run: bool,
) -> ExportSummary {
    let mut summary = ExportSummary {
        planned: plan.items.len(),
        missing: plan.missing.len(),
        ..Default::default()
    };

    for item in &plan.items {
        let dest = dest_root.join(&item.relative_dest);
        if let Ok(meta) = std::fs::metadata(&dest)
            && meta.len() == item.size_bytes
        {
            summary.skipped_existing += 1;
            continue;
        }
        if dry_run {
            // Count what a live run would do; hardlinks report as links.
            if link {
                summary.linked += 1;
            } else {
                summary.copied += 1;
            }
            summary.bytes += item.size_bytes;
            continue;
        }
        if let Some(parent) = dest.parent()
            && let Err(e) = std::fs::create_dir_all(parent)
        {
            eprintln!("⚠️  {}: {}", parent.display(), e);
            summary.errors += 1;
            continue;
        }
        if link {
            match std::fs::hard_link(&item.source, &dest) {
                Ok(()) => {
                    summary.linked += 1;
                    summary.bytes += item.size_bytes;
                    continue;
                }
                Err(_) => { /* cross-device or unsupported — fall through to copy */ }
            }
        }
        match std::fs::copy(&item.source, &dest) {
            Ok(bytes) => {
                summary.copied += 1;
                summary.bytes += bytes;
            }
            Err(e) => {
                eprintln!("⚠️  {} → {}: {}", item.source.display(), dest.display(), e);
                summary.errors += 1;
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts_schema;

    /// Fresh v23 DB with one project/target and three graded images whose
    /// FileName points into `dir`.
    fn seed(dir: &Path) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ts_schema::apply_schema(&conn).unwrap();
        conn.execute_batch(
            "INSERT INTO project (Id, profileId, name) VALUES (1, 'p', 'Proj');
             INSERT INTO target (Id, name, active, ra, dec, epochcode, projectid)
             VALUES (1, 'M42/Trapezium', 1, 5.5, -5.4, 2, 1);",
        )
        .unwrap();
        for (id, name, status) in [
            (1, "acc_Ha_0001.fits", 1),
            (2, "rej_Ha_0002.fits", 2),
            (3, "pend_OIII_0003.fits", 0),
        ] {
            let path = dir.join(name);
            std::fs::write(&path, b"fitsdata").unwrap();
            let filter = if name.contains("OIII") { "OIII" } else { "Ha" };
            // Serialize via serde_json so Windows path backslashes are
            // escaped exactly as the plugin's own writer would (a format!()
            // string produced invalid JSON on Windows and emptied the plan).
            let metadata =
                serde_json::json!({ "FileName": path.display().to_string() }).to_string();
            conn.execute(
                "INSERT INTO acquiredimage (Id, projectId, targetId, acquireddate, filtername,
                 gradingStatus, metadata) VALUES (?1, 1, 1, 100, ?2, ?3, ?4)",
                rusqlite::params![id, filter, status, metadata],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn plan_excludes_rejects_and_optionally_includes_pending() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];

        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        assert_eq!(plan.items.len(), 1, "accepted only by default");
        assert_eq!(plan.items[0].image_id, 1);

        let plan = plan_export(
            &conn,
            &dirs,
            &ExportOptions {
                include_pending: true,
                ..Default::default()
            },
        )
        .unwrap();
        let ids: Vec<i32> = plan.items.iter().map(|i| i.image_id).collect();
        assert!(ids.contains(&1) && ids.contains(&3) && !ids.contains(&2));
    }

    #[test]
    fn layout_is_wbpp_style_with_sanitized_names() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        // Target "M42/Trapezium" must not create nested directories.
        assert_eq!(
            plan.items[0].relative_dest,
            PathBuf::from("M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits")
        );
    }

    #[test]
    fn execute_copies_then_skips_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let dest = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();

        let s1 = execute_plan(&plan, dest.path(), false, false);
        assert_eq!((s1.copied, s1.errors), (1, 0));
        assert!(dest
            .path()
            .join("M42_Trapezium/LIGHT/Ha/acc_Ha_0001.fits")
            .is_file());

        let s2 = execute_plan(&plan, dest.path(), false, false);
        assert_eq!(s2.skipped_existing, 1);
        assert_eq!(s2.copied, 0);
    }

    #[test]
    fn hardlink_mode_links_same_filesystem() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("out"); // same fs as sources
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        let s = execute_plan(&plan, &dest, true, false);
        assert_eq!((s.linked, s.copied, s.errors), (1, 0, 0));
    }

    #[test]
    fn missing_files_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        std::fs::remove_file(dir.path().join("acc_Ha_0001.fits")).unwrap();
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(&conn, &dirs, &ExportOptions::default()).unwrap();
        assert_eq!(plan.items.len(), 0);
        assert_eq!(plan.missing.len(), 1);
    }

    #[test]
    fn filter_name_restricts_selection() {
        let dir = tempfile::tempdir().unwrap();
        let conn = seed(dir.path());
        let dirs = vec![dir.path().to_string_lossy().into_owned()];
        let plan = plan_export(
            &conn,
            &dirs,
            &ExportOptions {
                include_pending: true,
                filter_name: Some("oiii".into()),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(plan.items.len(), 1);
        assert_eq!(plan.items[0].image_id, 3);
    }
}
