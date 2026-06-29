use anyhow::{Context, Result};
use clap::Parser;
use rusqlite::Connection;

use crate::cli::{Cli, Commands};
use crate::commands::{
    analyze_fits_and_compare, annotate_stars, benchmark_psf, dump_grading_results,
    filter_rejected_files, list_projects, list_targets, read_fits, regrade_images, show_images,
    stretch_to_png, update_grade,
};

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
                use crate::commands::sync::{
                    parse_status, resolve_db_path, sync_grades, SyncGradesOptions,
                };
                use crate::db_registry::DbRegistry;
                use rusqlite::OpenFlags;
                use std::path::{Path, PathBuf};

                // Load the registry only when a side might be a slug (i.e. isn't
                // already an existing file), so syncing two plain paths doesn't
                // create a registry file as a side effect.
                let need_registry = !Path::new(&from).is_file() || !Path::new(&to).is_file();
                let registry_obj = if need_registry {
                    let registry_path = match &registry {
                        Some(p) => PathBuf::from(p),
                        None => {
                            DbRegistry::default_path().context("resolving default registry path")?
                        }
                    };
                    Some(DbRegistry::load_or_init(&registry_path).with_context(|| {
                        format!("loading registry at {}", registry_path.display())
                    })?)
                } else {
                    None
                };

                let from_path = resolve_db_path(registry_obj.as_ref(), &from)?;
                let to_path = resolve_db_path(registry_obj.as_ref(), &to)?;

                // Refuse to "sync" a database with itself.
                let from_canon =
                    std::fs::canonicalize(&from_path).unwrap_or_else(|_| from_path.clone());
                let to_canon = std::fs::canonicalize(&to_path).unwrap_or_else(|_| to_path.clone());
                if from_canon == to_canon {
                    return Err(anyhow::anyhow!(
                        "Source and destination resolve to the same database ({}); nothing to sync",
                        from_canon.display()
                    ));
                }

                let status_filter = match status.as_deref() {
                    Some(s) => Some(parse_status(s)?),
                    None => None,
                };

                let src = Connection::open_with_flags(&from_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .with_context(|| format!("opening source database {}", from_path.display()))?;
                let dest = Connection::open_with_flags(&to_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .with_context(|| {
                        format!("opening destination database {}", to_path.display())
                    })?;

                let options = SyncGradesOptions {
                    status_filter,
                    project_filter: project,
                    target_filter: target,
                    dry_run,
                };

                let summary = sync_grades(&src, &dest, &options)?;

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
                    from_path.display(),
                    to_path.display(),
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
                use crate::commands::sync::{resolve_db_path, sync_pull, PullOptions, TableCounts};
                use crate::db_registry::DbRegistry;
                use rusqlite::OpenFlags;
                use std::path::{Path, PathBuf};

                // Load the registry only when a side might be a slug.
                let need_registry = !Path::new(&from).is_file() || !Path::new(&to).is_file();
                let registry_obj = if need_registry {
                    let registry_path = match &registry {
                        Some(p) => PathBuf::from(p),
                        None => {
                            DbRegistry::default_path().context("resolving default registry path")?
                        }
                    };
                    Some(DbRegistry::load_or_init(&registry_path).with_context(|| {
                        format!("loading registry at {}", registry_path.display())
                    })?)
                } else {
                    None
                };

                let from_path = resolve_db_path(registry_obj.as_ref(), &from)?;
                let to_path = resolve_db_path(registry_obj.as_ref(), &to)?;

                let from_canon =
                    std::fs::canonicalize(&from_path).unwrap_or_else(|_| from_path.clone());
                let to_canon = std::fs::canonicalize(&to_path).unwrap_or_else(|_| to_path.clone());
                if from_canon == to_canon {
                    return Err(anyhow::anyhow!(
                        "Source and destination resolve to the same database ({}); nothing to pull",
                        from_canon.display()
                    ));
                }

                let src = Connection::open_with_flags(&from_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
                    .with_context(|| format!("opening source database {}", from_path.display()))?;
                let dest = Connection::open_with_flags(&to_path, OpenFlags::SQLITE_OPEN_READ_WRITE)
                    .with_context(|| {
                        format!("opening destination database {}", to_path.display())
                    })?;

                let options = PullOptions {
                    dry_run,
                    with_image_data: !no_image_data,
                    project_filter: project,
                };

                let summary = sync_pull(&src, &dest, &options)?;

                if verbose && !summary.changes.is_empty() {
                    println!("Entity changes:");
                    for c in &summary.changes {
                        println!("  {}", c);
                    }
                }

                let tc = |label: &str, c: &TableCounts| {
                    println!(
                        "  {:<16} inserted={} updated={} unchanged={}",
                        label, c.inserted, c.updated, c.unchanged
                    );
                };
                println!(
                    "\nEntity pull {} {} → {}:",
                    if dry_run { "(dry-run)" } else { "(live)" },
                    from_path.display(),
                    to_path.display(),
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
        },
        Commands::Server {
            config,
            registry,
            database,
            image_dirs,
            static_dir,
            cache_dir,
            port,
            host: _host,
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
            app_config.merge_with_cli(None, None, port, cache_dir);

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
            let databases = db_registry.databases.clone();

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
                )
                .await
            })?;
        }
    }

    Ok(())
}
