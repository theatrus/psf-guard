//! Out-of-tree reject archive support.
//!
//! Owns the `psf_guard_archive` sibling table inside the Target Scheduler
//! database. Each row records that psf-guard moved a rejected FITS file
//! (and its same-stem sidecars) out of the tree PixInsight loads in bulk,
//! keyed on the upstream `acquiredimage.guid` so it stays joinable across
//! TS exports/reimports.
//!
//! The plan, history, and design rationale live in
//! [REJECT_ARCHIVE_PLAN.md](../../../REJECT_ARCHIVE_PLAN.md). Phase A1 is
//! this module: schema bootstrap + read helpers + a schema-version guard.
//! Subsequent phases add destination computation (A3), sidecar discovery
//! (A4), the `move-rejects` CLI handler (A5), and an integration test (A7).

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};

use crate::commands::filter_rejected::get_possible_paths;
use crate::db::{Database, SchemaCapabilities};
use crate::db_registry::RejectArchiveOverrides;
use crate::directory_tree::DirectoryTree;
use crate::models::GradingStatus;

/// Compiled-in defaults for the reject-archive feature. Overridden by
/// per-DB registry fields (`DbEntry.reject_archive`), then by per-invocation
/// CLI flags. See REJECT_ARCHIVE_PLAN.md §4.2 for the precedence rules.
pub const DEFAULT_SEGMENT_NAME: &str = "REJECT";
pub const DEFAULT_DEPTH: u32 = 1;
pub const DEFAULT_SIDECAR_EXTS: &[&str] = &[".xisf", ".json", ".txt"];

/// Resolved (CLI ∪ per-DB ∪ defaults) configuration used at command time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectArchiveConfig {
    pub segment_name: String,
    pub depth: u32,
    pub sidecar_exts: Vec<String>,
}

impl Default for RejectArchiveConfig {
    fn default() -> Self {
        Self {
            segment_name: DEFAULT_SEGMENT_NAME.to_string(),
            depth: DEFAULT_DEPTH,
            sidecar_exts: DEFAULT_SIDECAR_EXTS
                .iter()
                .map(|s| (*s).to_string())
                .collect(),
        }
    }
}

/// Compose the effective archive config. CLI overrides win, then per-DB
/// registry block, then compiled-in defaults. Each field resolves
/// independently — e.g. a CLI segment override doesn't reset the per-DB
/// sidecar list.
///
/// Validates the resolved `segment_name` is non-empty and contains no
/// path separators (we use it as a literal path component). Validates
/// `sidecar_exts` are non-empty and dot-prefixed. Depth has no upper
/// bound — extreme values just degrade to "deeper anchor than the file
/// has segments," which `archive_path_for` handles by falling back to
/// the depth-0 case (A3).
pub fn resolve_config(
    per_db: Option<&RejectArchiveOverrides>,
    cli_segment: Option<&str>,
    cli_depth: Option<u32>,
    cli_sidecar_exts: Option<&[String]>,
) -> Result<RejectArchiveConfig> {
    let defaults = RejectArchiveConfig::default();

    let segment_name = cli_segment
        .map(|s| s.to_string())
        .or_else(|| per_db.and_then(|o| o.segment_name.clone()))
        .unwrap_or(defaults.segment_name);
    validate_segment_name(&segment_name)?;

    let depth = cli_depth
        .or_else(|| per_db.and_then(|o| o.depth))
        .unwrap_or(defaults.depth);

    let sidecar_exts = cli_sidecar_exts
        .map(|s| s.to_vec())
        .or_else(|| per_db.and_then(|o| o.sidecar_exts.clone()))
        .unwrap_or(defaults.sidecar_exts);
    for ext in &sidecar_exts {
        validate_sidecar_ext(ext)?;
    }

    Ok(RejectArchiveConfig {
        segment_name,
        depth,
        sidecar_exts,
    })
}

fn validate_segment_name(s: &str) -> Result<()> {
    if s.is_empty() {
        return Err(anyhow::anyhow!(
            "reject_archive.segment_name cannot be empty"
        ));
    }
    if s.contains('/') || s.contains('\\') {
        return Err(anyhow::anyhow!(
            "reject_archive.segment_name '{}' contains a path separator; \
             it must be a single directory name",
            s
        ));
    }
    if s == "." || s == ".." {
        return Err(anyhow::anyhow!(
            "reject_archive.segment_name '{}' is a special directory name",
            s
        ));
    }
    Ok(())
}

/// Compute the archive destination path for a single rejected file.
///
/// Returns `None` if `source_path` doesn't lie underneath `image_dir`, in
/// which case the caller should fall back to a different `image_dir` or
/// skip the file. Returns `Some(path)` otherwise; the path is purely
/// computed — no I/O, no parent-directory creation — so this function is
/// trivial to unit-test.
///
/// The rule: walk the source path's segments relative to `image_dir` and
/// insert `segment_name` after `depth` segments. With `depth = 1` and
/// `segment_name = "REJECT"` (the defaults):
///
/// ```text
/// image_dir = /Volumes/Astro/Targets
/// source    = /Volumes/Astro/Targets/M31/2026-04-16/B/LIGHT/img.fits
///                                    └── depth-1 anchor
/// archive   = /Volumes/Astro/Targets/M31/REJECT/2026-04-16/B/LIGHT/img.fits
/// ```
///
/// Edge cases:
/// - File is shallower than `depth` (e.g. depth=1 but the file lives
///   directly under `image_dir`): falls back to
///   `<image_dir>/<segment>/<filename>`.
/// - File is `image_dir` itself (no relative path): returns `None`.
/// - `depth = 0`: equivalent to the shallow-fallback case for every file.
///
/// Validation of `segment_name` happens in `resolve_config`; this function
/// assumes the caller already passed a valid name. Path separators in
/// `segment_name` would produce a path that crosses out of `image_dir`,
/// which is exactly what the validator prevents.
pub fn archive_path_for(
    image_dir: &std::path::Path,
    source_path: &std::path::Path,
    depth: u32,
    segment_name: &str,
) -> Option<std::path::PathBuf> {
    use std::path::{Component, PathBuf};

    let relative = source_path.strip_prefix(image_dir).ok()?;

    // Only `Normal` segments count for depth math. A relative path stripped
    // from an absolute prefix typically yields all-Normal components, but
    // be defensive about `..` / `.` that could appear in pathological
    // inputs.
    let segments: Vec<&std::ffi::OsStr> = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(s) => Some(s),
            _ => None,
        })
        .collect();

    if segments.is_empty() {
        // source_path == image_dir; not a file we can archive.
        return None;
    }

    let depth = depth as usize;
    let mut archive = PathBuf::from(image_dir);

    if depth == 0 || segments.len() <= depth {
        // Either explicitly "drop into a single REJECT bucket per image_dir"
        // or the file is too shallow to honor the requested depth — both
        // collapse to the same shape: <image_dir>/<segment>/<relative>.
        archive.push(segment_name);
        for seg in &segments {
            archive.push(seg);
        }
    } else {
        for seg in &segments[..depth] {
            archive.push(seg);
        }
        archive.push(segment_name);
        for seg in &segments[depth..] {
            archive.push(seg);
        }
    }

    Some(archive)
}

/// Find sidecar files in the same directory as `primary` whose stem matches
/// the primary's and whose extension is one of `exts`. Extension comparison
/// is case-insensitive; entries in `exts` must start with a `.` (validated
/// upstream in `resolve_config`).
///
/// Returns absolute paths sorted lexicographically for deterministic
/// dry-run output and stable archive-record contents. Returns an empty
/// vec — never errors — if the directory can't be read; the move
/// orchestrator handles missing primaries earlier.
///
/// Calibration masters (`Bias_master.fits`, `Dark_*.fits`, etc.) have a
/// different stem and are therefore never selected, even if their
/// extension is in the list.
pub fn find_sidecars(primary: &std::path::Path, exts: &[String]) -> Vec<std::path::PathBuf> {
    let Some(parent) = primary.parent() else {
        return Vec::new();
    };
    let Some(stem) = primary.file_stem() else {
        return Vec::new();
    };
    let primary_filename = primary.file_name();

    // Normalize the configured extensions once for the membership check.
    let lc_exts: Vec<String> = exts
        .iter()
        .map(|e| e.trim_start_matches('.').to_ascii_lowercase())
        .collect();

    let entries = match std::fs::read_dir(parent) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };

    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().map(|t| !t.is_file()).unwrap_or(true) {
            continue;
        }
        // Don't list the primary as its own sidecar.
        if primary_filename.is_some() && path.file_name() == primary_filename {
            continue;
        }
        if path.file_stem() != Some(stem) {
            continue;
        }
        let ext_lc = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase());
        let Some(ext_lc) = ext_lc else { continue };
        if lc_exts.contains(&ext_lc) {
            out.push(path);
        }
    }
    out.sort();
    out
}

fn validate_sidecar_ext(ext: &str) -> Result<()> {
    if !ext.starts_with('.') {
        return Err(anyhow::anyhow!(
            "reject_archive.sidecar_exts entry '{}' must start with a dot \
             (e.g. \".xisf\")",
            ext
        ));
    }
    if ext.len() < 2 {
        return Err(anyhow::anyhow!(
            "reject_archive.sidecar_exts entry '{}' is too short",
            ext
        ));
    }
    if ext.contains('/') || ext.contains('\\') {
        return Err(anyhow::anyhow!(
            "reject_archive.sidecar_exts entry '{}' contains a path separator",
            ext
        ));
    }
    Ok(())
}

/// `psf-guard`'s view of a single archived rejected image. One row per
/// `acquired_image_guid` (upstream's stable cross-tool join key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveRecord {
    pub acquired_image_guid: String,
    pub acquired_image_id: i64,
    /// Unix seconds (UTC) at which the move was committed to the DB.
    pub moved_at: i64,
    pub original_path: String,
    pub archive_path: String,
    /// Folder segment inserted between project and the rest of the path.
    /// Recorded so a future `restore-rejects` can rebuild the move plan
    /// even if the per-DB config changed since the move ran.
    pub segment_name: String,
    /// Depth at which `segment_name` was inserted. Same rationale.
    pub archive_depth: u32,
    /// Sidecar filenames (basename only, relative to the archive directory)
    /// that travelled alongside the primary. Serialized as a JSON array of
    /// strings in storage; deserialized eagerly here for ergonomics.
    pub sidecar_files: Vec<String>,
    /// Which registry slug owns this DB at the time of the move. Optional
    /// for forward-compatibility with non-multi-DB callers; in practice
    /// the v1 CLI always populates it.
    pub source_db_slug: Option<String>,
}

/// Create the archive table + index if they don't already exist.
///
/// Safe to call repeatedly — the statements are `IF NOT EXISTS`. Schema is
/// owned by psf-guard; no migrations from upstream Target Scheduler touch
/// it. See REJECT_ARCHIVE_PLAN.md §4.4 for the rationale.
pub fn ensure_archive_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS psf_guard_archive (
            acquired_image_guid TEXT PRIMARY KEY,
            acquired_image_id   INTEGER NOT NULL,
            moved_at            INTEGER NOT NULL,
            original_path       TEXT NOT NULL,
            archive_path        TEXT NOT NULL,
            segment_name        TEXT NOT NULL,
            archive_depth       INTEGER NOT NULL,
            sidecar_files       TEXT NOT NULL DEFAULT '[]',
            source_db_slug      TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_psf_guard_archive_image_id
            ON psf_guard_archive(acquired_image_id);
        "#,
    )
    .context("creating psf_guard_archive table")?;
    Ok(())
}

/// Refuse to operate against a Target Scheduler database that pre-dates the
/// `acquiredimage.guid` column (migration 22). Without `guid`, we have no
/// stable cross-export key to anchor archive rows against; falling back to
/// `Id` would silently desync after any TS DB export/reimport.
///
/// The error message is user-facing — keep it actionable.
pub fn require_target_scheduler_guid(conn: &Connection) -> Result<()> {
    let caps = SchemaCapabilities::detect(conn);
    if !caps.has_acquiredimage_guid {
        return Err(anyhow::anyhow!(
            "This Target Scheduler database is too old: it lacks the \
             `acquiredimage.guid` column (added in plugin schema v22) which \
             psf-guard's reject-archive feature uses to track moves across \
             DB exports/reimports.\n\nUpgrade by opening the database in a \
             recent N.I.N.A. + Target Scheduler version, or run earlier \
             psf-guard commands (`filter-rejected`) that don't need it."
        ));
    }
    Ok(())
}

/// Look up the archive record for an image by its TS guid. Returns
/// `Ok(None)` if the image was never archived by psf-guard.
pub fn get_archive_record_by_guid(conn: &Connection, guid: &str) -> Result<Option<ArchiveRecord>> {
    conn.query_row(
        "SELECT acquired_image_guid, acquired_image_id, moved_at,
                original_path, archive_path, segment_name, archive_depth,
                sidecar_files, source_db_slug
         FROM psf_guard_archive
         WHERE acquired_image_guid = ?1",
        params![guid],
        row_to_record,
    )
    .optional()
    .context("querying psf_guard_archive by guid")
}

/// Look up the archive record by the TS internal `acquiredimage.Id`. Slightly
/// less stable than guid (auto-increment IDs renumber on export/reimport)
/// but useful for in-process callers that already have the row id from a
/// query.
pub fn get_archive_record_by_image_id(
    conn: &Connection,
    image_id: i64,
) -> Result<Option<ArchiveRecord>> {
    conn.query_row(
        "SELECT acquired_image_guid, acquired_image_id, moved_at,
                original_path, archive_path, segment_name, archive_depth,
                sidecar_files, source_db_slug
         FROM psf_guard_archive
         WHERE acquired_image_id = ?1",
        params![image_id],
        row_to_record,
    )
    .optional()
    .context("querying psf_guard_archive by image_id")
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ArchiveRecord> {
    let sidecar_raw: String = row.get("sidecar_files")?;
    let sidecar_files = serde_json::from_str::<Vec<String>>(&sidecar_raw).unwrap_or_default();
    Ok(ArchiveRecord {
        acquired_image_guid: row.get("acquired_image_guid")?,
        acquired_image_id: row.get("acquired_image_id")?,
        moved_at: row.get("moved_at")?,
        original_path: row.get("original_path")?,
        archive_path: row.get("archive_path")?,
        segment_name: row.get("segment_name")?,
        archive_depth: row.get::<_, i64>("archive_depth")? as u32,
        sidecar_files,
        source_db_slug: row.get("source_db_slug")?,
    })
}

// ── Orchestration ─────────────────────────────────────────────────────────────

/// Options bag for `move_rejects`. Mirrors the CLI flags + per-DB context
/// the handler in `cli_main.rs` collects.
#[derive(Debug, Clone)]
pub struct MoveRejectsOptions {
    pub config: RejectArchiveConfig,
    pub project_filter: Option<String>,
    pub target_filter: Option<String>,
    pub dry_run: bool,
    /// Registry slug for the DB being operated on. Stored in each archive
    /// row so cross-DB tooling can later identify which rig produced a move.
    pub source_db_slug: String,
    /// Verbose: print per-image "tried this path, then that path" trace.
    pub verbose: bool,
}

/// Summary counters returned from a `move_rejects` run. The CLI prints them
/// at the end; tests assert on them directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MoveRejectsSummary {
    pub planned: usize,
    pub archived: usize,
    pub already_archived: usize,
    pub not_found_on_disk: usize,
    pub errors: usize,
}

/// Run the move-rejects pipeline. Locates each rejected image on disk under
/// one of `image_dirs`, computes the archive path, moves the primary plus
/// any matching sidecars, then records the move both in the
/// `psf_guard_archive` table and in a per-archive-root JSON manifest.
///
/// Returns a summary of what happened. Does not panic on per-image errors —
/// it counts them in `summary.errors` and moves on, so a batch of 100 with
/// one bad row still archives the other 99.
///
/// The function assumes:
/// - `ensure_archive_schema(conn)` has already been called (caller does).
/// - `require_target_scheduler_guid(conn)` has passed (caller does).
/// - `options.config` is valid (resolved through `resolve_config`).
pub fn move_rejects(
    conn: &Connection,
    image_dirs: &[String],
    options: &MoveRejectsOptions,
) -> Result<MoveRejectsSummary> {
    let db = Database::new(conn);

    let images = db
        .query_images(
            Some(GradingStatus::Rejected),
            options.project_filter.as_deref(),
            options.target_filter.as_deref(),
            None,
        )
        .context("querying rejected images")?;

    let mut summary = MoveRejectsSummary {
        planned: images.len(),
        ..Default::default()
    };

    // Build a directory tree per image_dir (lazily). Each tree caches the
    // first-hit basename → absolute-path mapping that powers the fallback
    // lookup when the date-aware path heuristics miss.
    let mut trees: Vec<DirectoryTree> = Vec::with_capacity(image_dirs.len());
    for dir in image_dirs {
        match DirectoryTree::build_multiple(&[std::path::Path::new(dir)]) {
            Ok(t) => trees.push(t),
            Err(e) => {
                eprintln!(
                    "⚠️  Could not index image directory {} ({}); continuing without it",
                    dir, e
                );
            }
        }
    }

    for (image, _project_name, target_name) in images {
        let Some(guid) = image.guid.as_deref() else {
            eprintln!(
                "⚠️  Skipping image id={} (rejected={}): no guid on row; \
                 cannot archive without a stable key. Upgrade the TS plugin \
                 to populate guid, or set the image to pending and re-grade.",
                image.id,
                image.reject_reason.as_deref().unwrap_or("")
            );
            summary.errors += 1;
            continue;
        };

        // Already archived? Skip silently (idempotent re-runs).
        if let Some(prior) = get_archive_record_by_guid(conn, guid)? {
            if options.verbose {
                println!("⏭️  {} already archived at {}", guid, prior.archive_path);
            }
            summary.already_archived += 1;
            continue;
        }

        // Extract filename from the metadata JSON.
        let filename = match parse_filename_from_metadata(&image.metadata) {
            Some(name) => name,
            None => {
                eprintln!(
                    "⚠️  Image id={} (guid={}) has no FileName in metadata; skipping",
                    image.id, guid
                );
                summary.errors += 1;
                continue;
            }
        };

        // Locate on disk. Try each image_dir's get_possible_paths first,
        // then fall back to the directory tree.
        let mut located: Option<(String, PathBuf)> = None;
        let date_str = image
            .acquired_date
            .and_then(|d| chrono::DateTime::from_timestamp(d, 0))
            .map(|dt| dt.format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        for (dir_idx, dir) in image_dirs.iter().enumerate() {
            for candidate in get_possible_paths(dir, &date_str, &target_name, &filename) {
                if candidate.exists() {
                    located = Some((dir.clone(), candidate));
                    break;
                }
            }
            if located.is_none() {
                if let Some(tree) = trees.get(dir_idx) {
                    if let Some(path) = tree.find_file_first(&filename) {
                        if path.exists() {
                            located = Some((dir.clone(), path.clone()));
                        }
                    }
                }
            }
            if located.is_some() {
                break;
            }
        }

        let Some((image_dir, source_path)) = located else {
            if options.verbose {
                eprintln!(
                    "⚠️  Not found on disk: id={} guid={} filename={}",
                    image.id, guid, filename
                );
            }
            summary.not_found_on_disk += 1;
            continue;
        };

        // Compute destination.
        let archive_path = match archive_path_for(
            Path::new(&image_dir),
            &source_path,
            options.config.depth,
            &options.config.segment_name,
        ) {
            Some(p) => p,
            None => {
                eprintln!(
                    "⚠️  Cannot compute archive path for {}: source not under image_dir {}",
                    source_path.display(),
                    image_dir
                );
                summary.errors += 1;
                continue;
            }
        };

        let sidecars = find_sidecars(&source_path, &options.config.sidecar_exts);

        // Print plan line whether dry-run or not (always; users want to see
        // what we did, not just what we will do).
        let kind = if options.dry_run { "PLAN" } else { "MOVE" };
        println!(
            "{}  {} → {}",
            kind,
            source_path.display(),
            archive_path.display()
        );
        for sc in &sidecars {
            println!("       + sidecar {}", sc.display());
        }

        if options.dry_run {
            continue;
        }

        // Execute. Roll back any partial moves for this image if anything
        // mid-sequence fails.
        let ctx = MoveContext {
            guid,
            image_id: image.id,
            source_path: &source_path,
            archive_path: &archive_path,
            sidecars: &sidecars,
            config: &options.config,
            source_db_slug: &options.source_db_slug,
        };
        match execute_one_move(conn, &ctx) {
            Ok(()) => summary.archived += 1,
            Err(e) => {
                eprintln!(
                    "❌ Failed to archive id={} guid={}: {:#}",
                    image.id, guid, e
                );
                summary.errors += 1;
            }
        }
    }

    Ok(summary)
}

fn parse_filename_from_metadata(metadata_json: &str) -> Option<String> {
    let v = serde_json::from_str::<serde_json::Value>(metadata_json).ok()?;
    let raw = v.get("FileName")?.as_str()?;
    // FileName values can be full paths from NINA; we only want the basename
    // (matches what `filter_rejected` does today).
    let basename = raw.split(&['\\', '/'][..]).next_back().unwrap_or(raw);
    Some(basename.to_string())
}

/// Per-image move context — kept as a struct rather than positional args so
/// `execute_one_move` and `append_to_manifest` stay below clippy's
/// too-many-arguments threshold and the call sites can't get fields mixed up.
struct MoveContext<'a> {
    guid: &'a str,
    image_id: i32,
    source_path: &'a Path,
    archive_path: &'a Path,
    sidecars: &'a [PathBuf],
    config: &'a RejectArchiveConfig,
    source_db_slug: &'a str,
}

fn execute_one_move(conn: &Connection, ctx: &MoveContext<'_>) -> Result<()> {
    let MoveContext {
        guid,
        image_id,
        source_path,
        archive_path,
        sidecars,
        config,
        source_db_slug,
    } = *ctx;
    let archive_parent = archive_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("archive_path has no parent: {:?}", archive_path))?;
    std::fs::create_dir_all(archive_parent)
        .with_context(|| format!("creating archive parent {}", archive_parent.display()))?;

    // Track what we moved so we can undo on later failure.
    let mut moved: Vec<(PathBuf, PathBuf)> = Vec::new();

    // Move the primary first.
    std::fs::rename(source_path, archive_path).with_context(|| {
        format!(
            "renaming primary {} → {}",
            source_path.display(),
            archive_path.display()
        )
    })?;
    moved.push((source_path.to_path_buf(), archive_path.to_path_buf()));

    // Sidecars next. Each lands beside the primary in the archive dir.
    let mut sidecar_names: Vec<String> = Vec::with_capacity(sidecars.len());
    for src in sidecars {
        let Some(name) = src.file_name() else {
            // Shouldn't happen (we got these from a read_dir), but bail out.
            rollback(&moved);
            return Err(anyhow::anyhow!(
                "sidecar path has no filename: {}",
                src.display()
            ));
        };
        let dest = archive_parent.join(name);
        if let Err(e) = std::fs::rename(src, &dest) {
            rollback(&moved);
            return Err(anyhow::Error::new(e).context(format!(
                "renaming sidecar {} → {}",
                src.display(),
                dest.display()
            )));
        }
        sidecar_names.push(name.to_string_lossy().into_owned());
        moved.push((src.clone(), dest));
    }

    // Record in the DB. If this insert fails (e.g. constraint violation),
    // also roll the moves back so the on-disk state matches the DB.
    let sidecar_json = serde_json::to_string(&sidecar_names).unwrap_or_else(|_| "[]".to_string());
    let now = chrono::Utc::now().timestamp();
    if let Err(e) = conn.execute(
        "INSERT INTO psf_guard_archive
         (acquired_image_guid, acquired_image_id, moved_at,
          original_path, archive_path, segment_name, archive_depth,
          sidecar_files, source_db_slug)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            guid,
            image_id,
            now,
            source_path.to_string_lossy().as_ref(),
            archive_path.to_string_lossy().as_ref(),
            &config.segment_name,
            config.archive_depth_as_i64(),
            sidecar_json,
            source_db_slug,
        ],
    ) {
        rollback(&moved);
        return Err(anyhow::Error::new(e).context("inserting psf_guard_archive row"));
    }

    // Manifest at the archive root (best-effort; if it fails, we log but
    // don't roll back — the DB row is the authoritative source).
    let manifest_ctx = ManifestEntryInput {
        guid,
        image_id,
        moved_at: now,
        original_path: source_path,
        archive_path,
        sidecar_files: &sidecar_names,
        config,
    };
    if let Err(e) = append_to_manifest(archive_parent, &manifest_ctx) {
        eprintln!(
            "⚠️  Wrote DB row but could not append manifest at {}: {:#}",
            archive_parent.display(),
            e
        );
    }

    Ok(())
}

fn rollback(moved: &[(PathBuf, PathBuf)]) {
    for (src, dest) in moved.iter().rev() {
        if let Err(e) = std::fs::rename(dest, src) {
            eprintln!(
                "❌ Rollback also failed: could not restore {} ← {}: {}",
                src.display(),
                dest.display(),
                e
            );
        }
    }
}

impl RejectArchiveConfig {
    fn archive_depth_as_i64(&self) -> i64 {
        self.depth as i64
    }
}

// ── Manifest helper ──────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize, serde::Deserialize, Default)]
struct ManifestFile {
    #[serde(default = "manifest_version_default")]
    version: u32,
    #[serde(default)]
    moves: Vec<ManifestEntry>,
}

fn manifest_version_default() -> u32 {
    1
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct ManifestEntry {
    guid: String,
    image_id: i32,
    moved_at: i64,
    original_path: String,
    archive_path: String,
    sidecar_files: Vec<String>,
    segment_name: String,
    archive_depth: u32,
}

struct ManifestEntryInput<'a> {
    guid: &'a str,
    image_id: i32,
    moved_at: i64,
    original_path: &'a Path,
    archive_path: &'a Path,
    sidecar_files: &'a [String],
    config: &'a RejectArchiveConfig,
}

/// Atomically append one entry to the manifest at the archive root. Reads
/// the existing file (if any), appends, writes to `.tmp`, then renames.
fn append_to_manifest(archive_root: &Path, entry: &ManifestEntryInput<'_>) -> Result<()> {
    let ManifestEntryInput {
        guid,
        image_id,
        moved_at,
        original_path,
        archive_path,
        sidecar_files,
        config,
    } = *entry;

    let manifest_path = archive_root.join(".psf-guard-manifest.json");
    let mut manifest: ManifestFile = if manifest_path.exists() {
        let body = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("reading {}", manifest_path.display()))?;
        serde_json::from_str(&body).unwrap_or_default()
    } else {
        ManifestFile {
            version: 1,
            moves: Vec::new(),
        }
    };
    manifest.moves.push(ManifestEntry {
        guid: guid.to_string(),
        image_id,
        moved_at,
        original_path: original_path.to_string_lossy().into_owned(),
        archive_path: archive_path.to_string_lossy().into_owned(),
        sidecar_files: sidecar_files.to_vec(),
        segment_name: config.segment_name.clone(),
        archive_depth: config.depth,
    });

    let tmp = manifest_path.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(&manifest).context("serializing manifest")?;
    std::fs::write(&tmp, body).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &manifest_path).context("renaming manifest tmp into place")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open_with_acquiredimage(guid: bool) -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        let guid_col = if guid { ", guid TEXT" } else { "" };
        conn.execute_batch(&format!(
            "CREATE TABLE acquiredimage (
                Id INTEGER PRIMARY KEY,
                projectId INTEGER NOT NULL,
                targetId INTEGER NOT NULL,
                gradingStatus INTEGER NOT NULL DEFAULT 0,
                metadata TEXT NOT NULL DEFAULT '{{}}'{guid_col}
            );",
        ))
        .unwrap();
        conn
    }

    #[test]
    fn ensure_archive_schema_is_idempotent_and_creates_index() {
        let conn = open_with_acquiredimage(true);
        // Call twice; second call must not error.
        ensure_archive_schema(&conn).unwrap();
        ensure_archive_schema(&conn).unwrap();

        // Table exists.
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='table' AND name='psf_guard_archive'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(table_count, 1);

        // Index exists.
        let idx_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master \
                 WHERE type='index' AND name='idx_psf_guard_archive_image_id'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(idx_count, 1);
    }

    #[test]
    fn require_target_scheduler_guid_errors_without_column() {
        let conn = open_with_acquiredimage(false);
        let err = require_target_scheduler_guid(&conn).unwrap_err();
        let msg = format!("{err}");
        // Both keywords appear in the message so users grepping logs can find it.
        assert!(msg.contains("guid"), "msg should mention guid: {msg}");
        assert!(
            msg.contains("v22") || msg.contains("22"),
            "msg should mention schema version: {msg}"
        );
    }

    #[test]
    fn require_target_scheduler_guid_passes_with_column() {
        let conn = open_with_acquiredimage(true);
        require_target_scheduler_guid(&conn).unwrap();
    }

    #[test]
    fn lookup_returns_none_when_no_row() {
        let conn = open_with_acquiredimage(true);
        ensure_archive_schema(&conn).unwrap();
        assert!(get_archive_record_by_guid(&conn, "nope").unwrap().is_none());
        assert!(get_archive_record_by_image_id(&conn, 999)
            .unwrap()
            .is_none());
    }

    #[test]
    fn lookup_returns_record_after_insert() {
        let conn = open_with_acquiredimage(true);
        ensure_archive_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_archive
             (acquired_image_guid, acquired_image_id, moved_at,
              original_path, archive_path, segment_name, archive_depth,
              sidecar_files, source_db_slug)
             VALUES ('abc', 42, 1700000000,
                     '/src/img.fits', '/src/REJECT/img.fits',
                     'REJECT', 1, '[\"img.xisf\"]', 'imaging-rig')",
            [],
        )
        .unwrap();

        let by_guid = get_archive_record_by_guid(&conn, "abc").unwrap().unwrap();
        assert_eq!(by_guid.acquired_image_id, 42);
        assert_eq!(by_guid.moved_at, 1700000000);
        assert_eq!(by_guid.original_path, "/src/img.fits");
        assert_eq!(by_guid.archive_path, "/src/REJECT/img.fits");
        assert_eq!(by_guid.segment_name, "REJECT");
        assert_eq!(by_guid.archive_depth, 1);
        assert_eq!(by_guid.sidecar_files, vec!["img.xisf"]);
        assert_eq!(by_guid.source_db_slug.as_deref(), Some("imaging-rig"));

        let by_id = get_archive_record_by_image_id(&conn, 42).unwrap().unwrap();
        assert_eq!(by_id, by_guid);
    }

    #[test]
    fn resolve_config_uses_defaults_when_nothing_overrides() {
        let cfg = resolve_config(None, None, None, None).unwrap();
        assert_eq!(cfg.segment_name, "REJECT");
        assert_eq!(cfg.depth, 1);
        assert_eq!(cfg.sidecar_exts, vec![".xisf", ".json", ".txt"]);
    }

    #[test]
    fn resolve_config_per_db_overrides_defaults() {
        let per_db = RejectArchiveOverrides {
            segment_name: Some("BAD".into()),
            depth: Some(2),
            sidecar_exts: Some(vec![".xisf".into()]),
        };
        let cfg = resolve_config(Some(&per_db), None, None, None).unwrap();
        assert_eq!(cfg.segment_name, "BAD");
        assert_eq!(cfg.depth, 2);
        assert_eq!(cfg.sidecar_exts, vec![".xisf"]);
    }

    #[test]
    fn resolve_config_cli_wins_over_per_db_per_field() {
        // CLI provides segment only; depth + sidecars fall through to per-DB.
        let per_db = RejectArchiveOverrides {
            segment_name: Some("BAD".into()),
            depth: Some(2),
            sidecar_exts: Some(vec![".xisf".into()]),
        };
        let cli_exts: Option<&[String]> = None;
        let cfg = resolve_config(Some(&per_db), Some("KEPT-AWAY"), None, cli_exts).unwrap();
        assert_eq!(cfg.segment_name, "KEPT-AWAY", "CLI segment wins");
        assert_eq!(
            cfg.depth, 2,
            "per-DB depth wins when CLI doesn't supply one"
        );
        assert_eq!(
            cfg.sidecar_exts,
            vec![".xisf"],
            "per-DB exts win when CLI doesn't supply"
        );
    }

    #[test]
    fn resolve_config_rejects_invalid_segment() {
        for bad in ["", "has/slash", "has\\backslash", ".", ".."] {
            let err = resolve_config(None, Some(bad), None, None).unwrap_err();
            let msg = format!("{err}");
            assert!(
                msg.contains("segment_name"),
                "error for {:?} should name the field: {msg}",
                bad
            );
        }
    }

    #[test]
    fn resolve_config_rejects_invalid_sidecar_exts() {
        let bad_exts: Vec<String> = vec!["xisf".into()]; // missing dot
        let err = resolve_config(None, None, None, Some(&bad_exts)).unwrap_err();
        assert!(format!("{err}").contains("sidecar_exts"));
    }

    fn p(s: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(s)
    }

    #[test]
    fn archive_path_for_inserts_at_depth_1_by_default() {
        let archive = archive_path_for(
            &p("/Volumes/Astro/Targets"),
            &p("/Volumes/Astro/Targets/M31/2026-04-16/B/LIGHT/img.fits"),
            1,
            "REJECT",
        )
        .unwrap();
        assert_eq!(
            archive,
            p("/Volumes/Astro/Targets/M31/REJECT/2026-04-16/B/LIGHT/img.fits")
        );
    }

    #[test]
    fn archive_path_for_handles_depth_2() {
        let archive = archive_path_for(
            &p("/data"),
            &p("/data/M31/2026-04-16/LIGHT/img.fits"),
            2,
            "REJECT",
        )
        .unwrap();
        assert_eq!(archive, p("/data/M31/2026-04-16/REJECT/LIGHT/img.fits"));
    }

    #[test]
    fn archive_path_for_treats_project_plus_file_as_in_tree() {
        // segments = ["M31", "img.fits"], depth = 1 → head=[M31], tail=[img.fits].
        // REJECT lands inside the project, NOT above it — keeps per-project
        // discoverability ("everything under M31 belongs to M31, including
        // its rejects").
        let archive = archive_path_for(&p("/data"), &p("/data/M31/img.fits"), 1, "REJECT").unwrap();
        assert_eq!(archive, p("/data/M31/REJECT/img.fits"));
    }

    #[test]
    fn archive_path_for_falls_back_when_file_is_at_image_dir_root() {
        // Only one segment under image_dir (just the file); depth=1 needs
        // depth+1=2 segments → fallback to image_dir/segment/file.
        let archive = archive_path_for(&p("/data"), &p("/data/img.fits"), 1, "REJECT").unwrap();
        assert_eq!(archive, p("/data/REJECT/img.fits"));
    }

    #[test]
    fn archive_path_for_falls_back_when_depth_exceeds_available_segments() {
        // Three segments under image_dir; depth=3 needs depth+1=4 → fallback.
        let archive = archive_path_for(
            &p("/data"),
            &p("/data/M31/2026-04-16/img.fits"),
            3,
            "REJECT",
        )
        .unwrap();
        assert_eq!(archive, p("/data/REJECT/M31/2026-04-16/img.fits"));
    }

    #[test]
    fn archive_path_for_depth_zero_drops_into_single_bucket() {
        let archive = archive_path_for(
            &p("/data"),
            &p("/data/M31/2026-04-16/LIGHT/img.fits"),
            0,
            "REJECT",
        )
        .unwrap();
        assert_eq!(archive, p("/data/REJECT/M31/2026-04-16/LIGHT/img.fits"));
    }

    #[test]
    fn archive_path_for_returns_none_when_source_outside_image_dir() {
        let archive = archive_path_for(&p("/data"), &p("/other/M31/img.fits"), 1, "REJECT");
        assert!(archive.is_none());
    }

    #[test]
    fn archive_path_for_returns_none_when_source_is_image_dir_itself() {
        let archive = archive_path_for(&p("/data"), &p("/data"), 1, "REJECT");
        assert!(archive.is_none());
    }

    #[test]
    fn archive_path_for_honors_custom_segment_name() {
        let archive = archive_path_for(
            &p("/data"),
            &p("/data/M31/2026-04-16/LIGHT/img.fits"),
            1,
            "PSF-Guard-Rejects",
        )
        .unwrap();
        assert_eq!(
            archive,
            p("/data/M31/PSF-Guard-Rejects/2026-04-16/LIGHT/img.fits")
        );
    }

    #[test]
    fn archive_path_for_handles_relative_paths_when_consistent() {
        // Both paths relative — practical for tests that work inside a
        // tempdir without needing absolute paths.
        let archive = archive_path_for(
            &p("targets"),
            &p("targets/M31/2026-04-16/LIGHT/img.fits"),
            1,
            "REJECT",
        )
        .unwrap();
        assert_eq!(archive, p("targets/M31/REJECT/2026-04-16/LIGHT/img.fits"));
    }

    #[test]
    fn find_sidecars_picks_same_stem_only() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("img_0028.fits");
        fs::write(&primary, b"").unwrap();

        // Sidecars (should be picked up):
        fs::write(dir.path().join("img_0028.xisf"), b"").unwrap();
        fs::write(dir.path().join("img_0028.json"), b"").unwrap();
        fs::write(dir.path().join("img_0028.txt"), b"").unwrap();

        // Same stem but extension not in the list:
        fs::write(dir.path().join("img_0028.log"), b"").unwrap();

        // Different stem — typical calibration-master shape:
        fs::write(dir.path().join("Bias_master.fits"), b"").unwrap();
        fs::write(dir.path().join("Dark_60s.xisf"), b"").unwrap();

        // Subdirectory should not be descended into:
        let sub = dir.path().join("nested");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("img_0028.xisf"), b"").unwrap();

        let exts: Vec<String> = [".xisf", ".json", ".txt"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let found = find_sidecars(&primary, &exts);

        let expected = vec![
            dir.path().join("img_0028.json"),
            dir.path().join("img_0028.txt"),
            dir.path().join("img_0028.xisf"),
        ];
        assert_eq!(found, expected);
    }

    #[test]
    fn find_sidecars_extension_match_is_case_insensitive() {
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("img.fits");
        fs::write(&primary, b"").unwrap();
        fs::write(dir.path().join("img.XISF"), b"").unwrap();
        fs::write(dir.path().join("img.Json"), b"").unwrap();

        let exts: Vec<String> = [".xisf", ".json"].iter().map(|s| s.to_string()).collect();
        let mut found = find_sidecars(&primary, &exts);
        found.sort();
        assert_eq!(found.len(), 2);
        assert!(found.iter().any(|p| p.file_name().unwrap() == "img.XISF"));
        assert!(found.iter().any(|p| p.file_name().unwrap() == "img.Json"));
    }

    #[test]
    fn find_sidecars_returns_empty_when_parent_missing() {
        let found = find_sidecars(
            std::path::Path::new("/no/such/dir/img.fits"),
            &[".xisf".to_string()],
        );
        assert!(found.is_empty());
    }

    #[test]
    fn find_sidecars_excludes_the_primary_itself() {
        // If exts somehow listed `.fits` (caller bug), the primary file
        // itself should still not be returned as its own sidecar.
        use std::fs;
        let dir = tempfile::tempdir().unwrap();
        let primary = dir.path().join("img.fits");
        fs::write(&primary, b"").unwrap();

        let exts = vec![".fits".to_string()];
        let found = find_sidecars(&primary, &exts);
        assert!(found.is_empty());
    }

    #[test]
    fn corrupt_sidecar_json_falls_back_to_empty() {
        let conn = open_with_acquiredimage(true);
        ensure_archive_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO psf_guard_archive
             (acquired_image_guid, acquired_image_id, moved_at,
              original_path, archive_path, segment_name, archive_depth,
              sidecar_files)
             VALUES ('x', 1, 0, '/o', '/a', 'REJECT', 1, 'not-json')",
            [],
        )
        .unwrap();
        let r = get_archive_record_by_guid(&conn, "x").unwrap().unwrap();
        assert!(r.sidecar_files.is_empty());
    }
}
