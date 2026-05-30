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
