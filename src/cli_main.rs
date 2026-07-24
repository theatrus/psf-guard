use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

use crate::cli::{Cli, Commands};
use crate::commands::{
    analyze_fits_and_compare, annotate_stars, benchmark_psf, dump_grading_results,
    filter_rejected_files, list_projects, list_targets, read_fits, regrade_images, screen_fits,
    show_images, stretch_to_png, update_grade,
};

struct SyncPair {
    from_path: PathBuf,
    to_path: PathBuf,
    source: Connection,
    destination: Connection,
}

/// Resolve registry slugs or file paths, reject self-sync, and open one
/// read-only source plus one existing read-write destination.
fn open_sync_pair(from: &str, to: &str, registry: Option<&str>) -> Result<SyncPair> {
    use crate::commands::sync::resolve_db_path;
    use crate::db_registry::DbRegistry;

    let need_registry = !Path::new(from).is_file() || !Path::new(to).is_file();
    let registry_obj = if need_registry {
        let registry_path = match registry {
            Some(path) => PathBuf::from(path),
            None => DbRegistry::default_path().context("resolving default registry path")?,
        };
        Some(
            DbRegistry::load_or_init(&registry_path)
                .with_context(|| format!("loading registry at {}", registry_path.display()))?,
        )
    } else {
        None
    };

    let from_path = resolve_db_path(registry_obj.as_ref(), from)?;
    let to_path = resolve_db_path(registry_obj.as_ref(), to)?;
    let from_canon = std::fs::canonicalize(&from_path).unwrap_or_else(|_| from_path.clone());
    let to_canon = std::fs::canonicalize(&to_path).unwrap_or_else(|_| to_path.clone());
    if from_canon == to_canon {
        return Err(anyhow::anyhow!(
            "Source and destination resolve to the same database ({}); nothing to sync",
            from_canon.display()
        ));
    }

    let source = Connection::open_with_flags(&from_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening source database {}", from_path.display()))?;
    let destination = Connection::open_with_flags(&to_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .with_context(|| format!("opening destination database {}", to_path.display()))?;

    Ok(SyncPair {
        from_path,
        to_path,
        source,
        destination,
    })
}

pub fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::DumpGrading {
            status,
            project,
            target,
            format,
        } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            dump_grading_results(&conn, status, project, target, &format)?;
        }
        Commands::ListProjects => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            list_projects(&conn)?;
        }
        Commands::RemoveImported {
            db,
            dry_run,
            registry,
        } => {
            use crate::db_registry::DbRegistry;
            use rusqlite::OpenFlags;
            use std::path::PathBuf;

            let registry_path = match &registry {
                Some(p) => PathBuf::from(p),
                None => DbRegistry::default_path().context("resolving default registry path")?,
            };
            let reg = DbRegistry::load_or_init(&registry_path).ok();
            let db_path = crate::commands::sync::resolve_db_path(reg.as_ref(), &db)?;
            let mut conn = Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
            )
            .with_context(|| format!("opening database at {}", db_path.display()))?;

            let (projects, targets, plans, images) =
                crate::commands::import::remove_imported(&mut conn, dry_run)?;
            println!(
                "Remove-imported {}: {} project(s), {} target(s), {} plan(s), {} image row(s)",
                if dry_run { "(dry-run)" } else { "(live)" },
                projects,
                targets,
                plans,
                images
            );
        }
        Commands::Export {
            db,
            dest,
            include_pending,
            project,
            target,
            filter,
            link,
            dry_run,
            image_dirs,
            registry,
        } => {
            use crate::commands::export::{execute_plan, plan_export, ExportOptions};
            use crate::db_registry::DbRegistry;
            use rusqlite::OpenFlags;
            use std::path::PathBuf;

            let registry_path = match &registry {
                Some(p) => PathBuf::from(p),
                None => DbRegistry::default_path().context("resolving default registry path")?,
            };
            let reg = DbRegistry::load_or_init(&registry_path).ok();
            let db_path = crate::commands::sync::resolve_db_path(reg.as_ref(), &db)?;
            // Image dirs: explicit flag > registry entry.
            let dirs = match image_dirs {
                Some(dirs) if !dirs.is_empty() => dirs,
                _ => reg
                    .as_ref()
                    .and_then(|r| r.find(&db))
                    .map(|e| e.image_dirs.clone())
                    .unwrap_or_default(),
            };
            if dirs.is_empty() {
                return Err(anyhow::anyhow!(
                    "No image directories: pass --image-dirs or use a registry slug \
                     with configured image_dirs."
                ));
            }

            let conn = Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
            )
            .with_context(|| format!("opening database at {}", db_path.display()))?;

            let options = ExportOptions {
                include_pending,
                project_filter: project,
                target_filter: target,
                filter_name: filter,
                ..Default::default()
            };
            let plan = plan_export(&conn, &dirs, &options)?;
            let summary = execute_plan(&plan, &PathBuf::from(&dest), link, dry_run);

            println!(
                "\nExport {}: planned={}, copied={}, linked={}, already_present={}, \
                 missing_files={}, errors={}, {:.2} GiB",
                if dry_run { "(dry-run)" } else { "(live)" },
                summary.planned,
                summary.copied,
                summary.linked,
                summary.skipped_existing,
                summary.missing,
                summary.errors,
                summary.bytes as f64 / (1024.0 * 1024.0 * 1024.0),
            );
            for (id, basename) in plan.missing.iter().take(10) {
                eprintln!("  ⚠️ missing on disk: image {} ({})", id, basename);
            }
            if plan.missing.len() > 10 {
                eprintln!("  … and {} more missing", plan.missing.len() - 10);
            }
            if summary.errors > 0 {
                return Err(anyhow::anyhow!(
                    "{} file(s) failed to export",
                    summary.errors
                ));
            }
        }
        Commands::CreateDb {
            database,
            directories,
            name,
            time_gap_days,
            profile_id,
            dry_run,
            no_register,
            registry,
        } => {
            use crate::commands::import::{
                collect_fits_files, import_frames, print_outcome, scan_frames, ImportOptions,
            };
            use crate::db_registry::DbRegistry;
            use std::path::PathBuf;

            let dirs: Vec<PathBuf> = directories.iter().map(PathBuf::from).collect();
            let files = collect_fits_files(&dirs)?;
            println!("Found {} FITS file(s); reading headers...", files.len());
            let frames = scan_frames(&files);

            let db_path = PathBuf::from(&database);
            let mut conn = crate::ts_schema::create_fresh_db(&db_path)?;
            println!(
                "Created Target Scheduler database at {} (schema v{})",
                db_path.display(),
                crate::ts_schema::TS_SCHEMA_VERSION
            );

            let options = ImportOptions {
                time_gap_days,
                profile_id,
                dry_run,
                ..Default::default()
            };
            let outcome = import_frames(&mut conn, frames, &options)?;
            print_outcome(&outcome);

            if dry_run {
                // The bootstrap itself is not rolled back — remove the file
                // so a dry run leaves nothing behind.
                drop(conn);
                std::fs::remove_file(&db_path)
                    .with_context(|| format!("removing dry-run database {}", db_path.display()))?;
                println!("Dry run: removed {}", db_path.display());
            } else if !no_register {
                let registry_path = match registry {
                    Some(p) => PathBuf::from(p),
                    None => {
                        DbRegistry::default_path().context("resolving default registry path")?
                    }
                };
                let mut reg = DbRegistry::load_or_init(&registry_path)
                    .with_context(|| format!("loading registry at {}", registry_path.display()))?;
                let display_name = name.unwrap_or_else(|| {
                    db_path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Imported".to_string())
                });
                let entry = reg
                    .add(
                        display_name,
                        db_path.to_string_lossy().into_owned(),
                        directories,
                        None,
                    )?
                    .clone();
                reg.save(&registry_path)?;
                println!(
                    "Registered as '{}' (slug {}) in {}",
                    entry.name,
                    entry.id,
                    registry_path.display()
                );
            }
        }
        Commands::Import {
            db,
            directories,
            time_gap_days,
            profile_id,
            dry_run,
            no_attach,
            match_radius_deg,
            registry,
        } => {
            use crate::commands::import::{
                collect_fits_files, import_frames, print_outcome, scan_frames, ImportOptions,
            };
            use crate::commands::sync::{require_pull_capable, resolve_db_path};
            use crate::db_registry::DbRegistry;
            use rusqlite::OpenFlags;
            use std::path::PathBuf;

            // Prefer a registry slug; fall back to a raw file path.
            let registry_path = match &registry {
                Some(p) => PathBuf::from(p),
                None => DbRegistry::default_path().context("resolving default registry path")?,
            };
            let reg = DbRegistry::load_or_init(&registry_path).ok();
            let db_path = resolve_db_path(reg.as_ref(), &db)?;

            let dirs: Vec<PathBuf> = directories.iter().map(PathBuf::from).collect();
            let files = collect_fits_files(&dirs)?;
            println!("Found {} FITS file(s); reading headers...", files.len());
            let frames = scan_frames(&files);

            // READ_WRITE without CREATE: a wrong path must error, not leave a
            // junk sqlite file behind (same rule as screen-fits --regrade-db).
            let mut conn = Connection::open_with_flags(
                &db_path,
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_URI,
            )
            .with_context(|| format!("opening database at {}", db_path.display()))?;
            // Import needs the same v22+ guid columns sync pull does.
            require_pull_capable(&conn)?;

            let options = ImportOptions {
                time_gap_days,
                profile_id,
                dry_run,
                attach_existing: !no_attach,
                match_radius_deg,
            };
            let outcome = import_frames(&mut conn, frames, &options)?;
            print_outcome(&outcome);
        }
        Commands::ListTargets { project } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            list_targets(&conn, &project)?;
        }
        Commands::MoveRejects {
            db,
            dry_run,
            reject_segment,
            reject_depth,
            sidecar_exts,
            registry,
            project,
            target,
            verbose,
        } => {
            use crate::commands::reject_archive::{
                ensure_archive_schema, move_rejects, require_target_scheduler_guid, resolve_config,
                MoveRejectsOptions,
            };
            use crate::db_registry::DbRegistry;
            use std::path::PathBuf;

            let registry_path = match registry {
                Some(p) => PathBuf::from(p),
                None => DbRegistry::default_path().context("resolving default registry path")?,
            };
            let db_registry = DbRegistry::load_or_init(&registry_path)
                .with_context(|| format!("loading registry at {}", registry_path.display()))?;
            let entry = db_registry
                .find(&db)
                .ok_or_else(|| anyhow::anyhow!(
                    "No database with slug '{}' in {} (use `psf-guard server` once to register, or hand-edit the file).",
                    db,
                    registry_path.display(),
                ))?;
            if entry.image_dirs.is_empty() {
                return Err(anyhow::anyhow!(
                    "Database '{}' has no image_dirs configured in the registry; \
                     archiving needs to know where to search for files.",
                    db
                ));
            }

            let conn = Connection::open(&entry.db_path)
                .with_context(|| format!("opening database at {}", entry.db_path))?;
            require_target_scheduler_guid(&conn)?;
            // Dry-run must not write to the DB — defer table creation to live
            // runs. move_rejects tolerates a missing table in dry-run mode.
            if !dry_run {
                ensure_archive_schema(&conn)?;
            }

            let resolved = resolve_config(
                entry.reject_archive.as_ref(),
                reject_segment.as_deref(),
                reject_depth,
                sidecar_exts.as_deref(),
            )?;

            let options = MoveRejectsOptions {
                config: resolved,
                project_filter: project,
                target_filter: target,
                dry_run,
                source_db_slug: entry.id.clone(),
                verbose,
            };

            let summary = move_rejects(&conn, &entry.image_dirs, &options)?;
            println!(
                "\nReject archive {}: planned={}, archived={}, already_archived={}, missing_archive={}, not_found={}, errors={}",
                if dry_run { "(dry-run)" } else { "(live)" },
                summary.planned,
                summary.archived,
                summary.already_archived,
                summary.missing_archive,
                summary.not_found_on_disk,
                summary.errors,
            );
        }

        Commands::RestoreRejects {
            db,
            all,
            image_id,
            guid,
            dry_run,
            registry,
            verbose,
        } => {
            use crate::commands::reject_archive::{
                require_target_scheduler_guid, restore_rejects, RestoreRejectsOptions,
            };
            use crate::db_registry::DbRegistry;
            use std::path::PathBuf;

            let registry_path = match registry {
                Some(p) => PathBuf::from(p),
                None => DbRegistry::default_path().context("resolving default registry path")?,
            };
            let db_registry = DbRegistry::load_or_init(&registry_path)
                .with_context(|| format!("loading registry at {}", registry_path.display()))?;
            let entry = db_registry
                .find(&db)
                .ok_or_else(|| anyhow::anyhow!(
                    "No database with slug '{}' in {} (use `psf-guard server` once to register, or hand-edit the file).",
                    db,
                    registry_path.display(),
                ))?;

            let conn = Connection::open(&entry.db_path)
                .with_context(|| format!("opening database at {}", entry.db_path))?;
            require_target_scheduler_guid(&conn)?;

            let options = RestoreRejectsOptions {
                restore_all: all,
                image_id_filter: image_id,
                guid_filter: guid,
                dry_run,
                verbose,
            };

            let summary = restore_rejects(&conn, &options)?;
            println!(
                "\nRestore rejects {}: planned={}, restored={} (with_suffix={}), skipped_still_rejected={}, missing_archive={}, errors={}",
                if dry_run { "(dry-run)" } else { "(live)" },
                summary.planned,
                summary.restored,
                summary.restored_with_suffix,
                summary.skipped_still_rejected,
                summary.missing_archive,
                summary.errors,
            );
        }

        Commands::FilterRejected {
            database,
            base_dir,
            dry_run,
            project,
            target,
            verbose,
            stat_options,
        } => {
            eprintln!(
                "⚠️  `filter-rejected` is deprecated. It renames `LIGHT/` → \
                 `LIGHT_REJECT/` as a sibling under the same project root, \
                 which PixInsight's bulk-load workflows still pick up.\n   \
                 For an out-of-tree archive with idempotency, sidecar moves, \
                 and a future restore path, register the DB in the registry \
                 (run `psf-guard server <db> <dirs>` once) and use:\n     \
                 psf-guard move-rejects --db <slug>\n   See \
                 REJECT_ARCHIVE_PLAN.md for the full design. `filter-rejected` \
                 still works for now and retains its statistical-analysis \
                 flags that the new command does not duplicate.\n"
            );
            let conn = Connection::open(&database)
                .with_context(|| format!("Failed to open database: {}", database))?;

            let stat_config = stat_options.to_grading_config();
            filter_rejected_files(
                &conn,
                &base_dir,
                dry_run,
                project,
                target,
                stat_config,
                verbose,
            )?;
        }
        Commands::Regrade {
            database,
            dry_run,
            target,
            project,
            days,
            reset,
            stat_options,
        } => {
            let conn = Connection::open(&database)
                .with_context(|| format!("Failed to open database: {}", database))?;

            let stat_config = stat_options.to_grading_config();
            regrade_images(&conn, dry_run, target, project, days, &reset, stat_config)?;
        }
        Commands::ShowImages { ids } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            show_images(&conn, &ids)?;
        }
        Commands::UpdateGrade { id, status, reason } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            update_grade(&conn, id, &status, reason)?;
        }
        Commands::ReadFits {
            path,
            verbose,
            format,
        } => {
            read_fits(&path, verbose, &format)?;
        }
        Commands::AnalyzeFits {
            path,
            project,
            target,
            format,
            detector,
            sensitivity,
            apply_stretch,
            compare_all,
            psf_type,
            verbose,
        } => {
            let conn = Connection::open(&cli.database)
                .with_context(|| format!("Failed to open database: {}", cli.database))?;
            analyze_fits_and_compare(
                &conn,
                &path,
                project,
                target,
                &format,
                &detector,
                &sensitivity,
                apply_stretch,
                compare_all,
                &psf_type,
                verbose,
            )?;
        }
        Commands::ScreenFits {
            path,
            detector,
            format,
            min_score,
            dead_cell_rise,
            threads,
            session_gap,
            regrade_db,
            dry_run,
            registry,
            cache_dir,
            annotate,
            verbose,
        } => {
            crate::debug::init_debug(verbose);
            let options = crate::commands::screen_fits::ScreenOptions {
                detector,
                format,
                min_score,
                dead_cell_rise,
                threads,
                session_gap_minutes: session_gap,
                regrade_db,
                dry_run,
                registry,
                cache_dir,
                annotate_dir: annotate,
            };
            screen_fits(&path, &options)?;
        }
        Commands::StretchToPng {
            fits_path,
            output,
            midtone_factor,
            shadow_clipping,
            logarithmic,
            invert,
        } => {
            stretch_to_png(
                &fits_path,
                output,
                midtone_factor,
                shadow_clipping,
                logarithmic,
                invert,
            )?;
        }
        Commands::AnnotateStars {
            fits_path,
            output,
            max_stars,
            detector,
            sensitivity,
            midtone_factor,
            shadow_clipping,
            annotation_color,
            psf_type,
            verbose,
        } => {
            annotate_stars(
                &fits_path,
                output,
                max_stars,
                &detector,
                &sensitivity,
                midtone_factor,
                shadow_clipping,
                &annotation_color,
                &psf_type,
                verbose,
            )?;
        }
        Commands::VisualizePsf {
            fits_path,
            output,
            star_index,
            psf_type,
            max_stars,
            selection_mode,
            sort_by,
            verbose,
        } => {
            use crate::commands::visualize_psf::visualize_psf_multi;

            // If a specific star index is requested, show just that one
            let num_stars = if star_index.is_some() { 1 } else { max_stars };

            visualize_psf_multi(
                &fits_path,
                output,
                num_stars,
                &psf_type,
                &sort_by,
                3, // Default to 3 columns
                &selection_mode,
                verbose,
            )?;
        }
        Commands::VisualizePsfMulti {
            fits_path,
            output,
            num_stars,
            psf_type,
            sort_by,
            grid_cols,
            selection_mode,
            verbose,
        } => {
            use crate::commands::visualize_psf::visualize_psf_multi;

            visualize_psf_multi(
                &fits_path,
                output,
                num_stars,
                &psf_type,
                &sort_by,
                grid_cols,
                &selection_mode,
                verbose,
            )?;
        }
        Commands::BenchmarkPsf {
            fits_path,
            runs,
            verbose,
        } => {
            benchmark_psf(&fits_path, runs, verbose)?;
        }
        Commands::Sync { kind } => match kind {
            crate::cli::SyncKind::Grades {
                from,
                to,
                dry_run,
                status,
                project,
                target,
                registry,
                verbose,
            } => {
                use crate::commands::sync::{parse_status, sync_grades, SyncGradesOptions};

                let status_filter = match status.as_deref() {
                    Some(s) => Some(parse_status(s)?),
                    None => None,
                };

                let pair = open_sync_pair(&from, &to, registry.as_deref())?;

                let options = SyncGradesOptions {
                    status_filter,
                    reviewed_only: false,
                    project_filter: project,
                    target_filter: target,
                    dry_run,
                };

                let summary = sync_grades(&pair.source, &pair.destination, &options)?;

                if verbose && !summary.changes.is_empty() {
                    use crate::models::GradingStatus;
                    println!("Grade transitions (dest → src):");
                    for c in &summary.changes {
                        println!(
                            "  {}  {} → {}{}",
                            c.guid,
                            GradingStatus::from_i32(c.from),
                            GradingStatus::from_i32(c.to),
                            c.reason
                                .as_deref()
                                .map(|r| format!("  [{}]", r))
                                .unwrap_or_default(),
                        );
                    }
                }

                println!(
                    "\nGrade sync {} {} → {}:",
                    if dry_run { "(dry-run)" } else { "(live)" },
                    pair.from_path.display(),
                    pair.to_path.display(),
                );
                println!(
                    "  source rows considered: {} (skipped, no guid: {})",
                    summary.source_considered, summary.source_no_guid
                );
                println!("  matched in destination: {}", summary.matched);
                println!(
                    "    changed:   {}{}",
                    summary.changed,
                    if dry_run { " (would change)" } else { "" }
                );
                println!("    unchanged: {}", summary.unchanged);
                println!(
                    "  source guid not in destination: {}",
                    summary.unmatched_source
                );
                println!("  destination guid not in source: {}", summary.dest_only);
                if summary.duplicate_guids > 0 {
                    println!("  duplicate guids skipped: {}", summary.duplicate_guids);
                }
                if !summary.transitions.is_empty() {
                    println!("  transitions:");
                    for (label, count) in &summary.transitions {
                        println!("    {}: {}", label, count);
                    }
                }
            }

            crate::cli::SyncKind::Pull {
                from,
                to,
                dry_run,
                no_image_data,
                project,
                registry,
                verbose,
            } => {
                use crate::commands::sync::{sync_pull, PullOptions, TableCounts};
                let pair = open_sync_pair(&from, &to, registry.as_deref())?;

                let options = PullOptions {
                    dry_run,
                    with_image_data: !no_image_data,
                    project_filter: project,
                };

                let summary = sync_pull(&pair.source, &pair.destination, &options)?;

                if verbose && !summary.changes.is_empty() {
                    println!("Entity changes:");
                    for c in &summary.changes {
                        println!("  {}", c);
                    }
                }

                let tc = |label: &str, c: &TableCounts| {
                    let skipped = if c.skipped > 0 {
                        format!(" skipped={}", c.skipped)
                    } else {
                        String::new()
                    };
                    println!(
                        "  {:<16} inserted={} updated={} unchanged={}{}",
                        label, c.inserted, c.updated, c.unchanged, skipped
                    );
                };
                println!(
                    "\nEntity pull {} {} → {}:",
                    if dry_run { "(dry-run)" } else { "(live)" },
                    pair.from_path.display(),
                    pair.to_path.display(),
                );
                tc("exposuretemplate", &summary.exposuretemplate);
                tc("project", &summary.project);
                tc("ruleweight", &summary.ruleweight);
                tc("target", &summary.target);
                tc("exposureplan", &summary.exposureplan);
                tc("acquiredimage", &summary.acquiredimage);
                println!(
                    "    grades: filled(pending→telescope)={} preserved(local)={}",
                    summary.grade_filled, summary.grade_preserved
                );
                if summary.imagedata_synced {
                    tc("imagedata", &summary.imagedata);
                } else {
                    println!("  imagedata        skipped (--no-image-data)");
                }

                // Human-readable byte size.
                let human = |n: u64| -> String {
                    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
                    let mut f = n as f64;
                    let mut i = 0;
                    while f >= 1024.0 && i < UNITS.len() - 1 {
                        f /= 1024.0;
                        i += 1;
                    }
                    if i == 0 {
                        format!("{} {}", n, UNITS[0])
                    } else {
                        format!("{:.1} {}", f, UNITS[i])
                    }
                };
                let suffix = if dry_run { " (planned)" } else { "" };
                println!(
                    "  ── total: inserted={} updated={}{}",
                    summary.total_inserted(),
                    summary.total_updated(),
                    suffix
                );
                if summary.imagedata_synced {
                    println!(
                        "     imagedata copied: {}{}",
                        human(summary.imagedata_bytes),
                        suffix
                    );
                }
            }

            crate::cli::SyncKind::Planning {
                from,
                to,
                dry_run,
                project,
                registry,
                verbose,
            } => {
                use crate::commands::sync::{sync_planning, PlanningOptions, TableCounts};

                let pair = open_sync_pair(&from, &to, registry.as_deref())?;
                let summary = sync_planning(
                    &pair.source,
                    &pair.destination,
                    &PlanningOptions {
                        dry_run,
                        project_filter: project,
                    },
                )?;

                if verbose {
                    for change in &summary.changes {
                        println!("  {}", change);
                    }
                }
                let print_counts = |label: &str, counts: &TableCounts| {
                    println!(
                        "  {:<16} inserted={} updated={} unchanged={} skipped={}",
                        label, counts.inserted, counts.updated, counts.unchanged, counts.skipped
                    );
                };
                println!(
                    "\nPlanning sync {} {} → {}:",
                    if dry_run { "(dry-run)" } else { "(live)" },
                    pair.from_path.display(),
                    pair.to_path.display(),
                );
                print_counts("exposuretemplate", &summary.exposuretemplate);
                print_counts("project", &summary.project);
                print_counts("ruleweight", &summary.ruleweight);
                print_counts("target", &summary.target);
                print_counts("exposureplan", &summary.exposureplan);
                println!(
                    "  ── total: inserted={} updated={}{}",
                    summary.total_inserted(),
                    summary.total_updated(),
                    if dry_run { " (planned)" } else { "" }
                );
                println!("     telescope capture counts, images, and grades were left unchanged");
            }
        },
        Commands::Server {
            config,
            registry,
            database,
            image_dirs,
            static_dir,
            cache_dir,
            port,
            host,
            pregenerate_screen,
            pregenerate_large,
            pregenerate_original,
            pregenerate_annotated,
            pregenerate_all,
            cache_expiry,
            allow_database_management,
        } => {
            use crate::config::Config;
            use crate::db_registry::DbRegistry;
            use std::path::PathBuf;

            // 1) Resolve and load the database registry.
            let registry_path = match registry {
                Some(p) => PathBuf::from(p),
                None => DbRegistry::default_path().context("resolving default registry path")?,
            };
            let mut db_registry = DbRegistry::load_or_init(&registry_path)
                .with_context(|| format!("loading registry at {}", registry_path.display()))?;

            // 2) If the user passed a positional DB, register it (idempotent).
            if let Some(db_path) = database {
                if db_registry.find_by_path(&db_path).is_none() {
                    let name = PathBuf::from(&db_path)
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "Database".to_string());
                    db_registry
                        .add(name, db_path.clone(), image_dirs.clone(), None)
                        .with_context(|| format!("registering {}", db_path))?;
                    db_registry.save(&registry_path).with_context(|| {
                        format!("saving registry to {}", registry_path.display())
                    })?;
                    eprintln!("Registered new database in {}", registry_path.display());
                } else if !image_dirs.is_empty() {
                    eprintln!(
                        "Database already registered; positional image dirs ignored \
                         (edit the registry to update them)"
                    );
                }
            }

            // 3) Load shared TOML config for port/cache_dir/pregeneration knobs.
            let mut app_config = if let Some(config_path) = config {
                Config::from_file(&config_path)
                    .with_context(|| format!("Failed to load config file: {}", config_path))?
            } else {
                Config::default()
            };
            app_config.merge_with_cli(None, None, port, host, cache_dir);

            // We deliberately do NOT call app_config.validate() — the DB path
            // requirement no longer applies (DBs come from the registry).

            use crate::cli::PregenerationConfig;
            let pregeneration_config = if pregenerate_all
                || pregenerate_screen
                || pregenerate_large
                || pregenerate_original
                || pregenerate_annotated
            {
                PregenerationConfig::from_server_args(
                    pregenerate_screen,
                    pregenerate_large,
                    pregenerate_original,
                    pregenerate_annotated,
                    pregenerate_all,
                    &cache_expiry,
                )?
            } else {
                PregenerationConfig::from_config(app_config.get_pregeneration())
            };

            let cache_directory = app_config.get_cache_directory();
            let server_host = app_config.get_host();
            let server_port = app_config.get_port();
            let worker_policy = app_config.get_worker_policy();
            let site_banner = app_config.get_site_banner()?;
            let databases = db_registry.databases.clone();
            let astrometry_config = db_registry.astrometry.clone();

            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                crate::server::run_server(
                    databases,
                    static_dir,
                    cache_directory,
                    server_host,
                    server_port,
                    pregeneration_config,
                    Some(registry_path),
                    allow_database_management,
                    site_banner,
                    worker_policy,
                    astrometry_config,
                )
                .await
            })?;
        }
    }

    Ok(())
}
