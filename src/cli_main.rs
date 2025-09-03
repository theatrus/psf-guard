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
        Commands::FilterRejected {
            database,
            base_dir,
            dry_run,
            project,
            target,
            verbose,
            stat_options,
        } => {
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
        } => {
            use crate::config::Config;

            // Load configuration from file or use defaults
            let mut app_config = if let Some(config_path) = config {
                Config::from_file(&config_path)
                    .with_context(|| format!("Failed to load config file: {}", config_path))?
            } else {
                Config::default()
            };

            // Override config with command line arguments
            let database_path = if database.is_some() || image_dirs.is_empty() {
                database
            } else {
                // If no database specified and we have image_dirs from CLI, use the default
                Some(app_config.database.path.clone())
            };

            let image_directories = if !image_dirs.is_empty() {
                Some(image_dirs)
            } else {
                None
            };

            app_config.merge_with_cli(database_path, image_directories, port, cache_dir);

            // Validate configuration
            app_config
                .validate()
                .context("Configuration validation failed")?;

            // Create pregeneration configuration
            use crate::cli::PregenerationConfig;
            let pregeneration_config = if pregenerate_all
                || pregenerate_screen
                || pregenerate_large
                || pregenerate_original
                || pregenerate_annotated
            {
                // CLI flags take precedence
                PregenerationConfig::from_server_args(
                    pregenerate_screen,
                    pregenerate_large,
                    pregenerate_original,
                    pregenerate_annotated,
                    pregenerate_all,
                    &cache_expiry,
                )?
            } else {
                // Use config file settings
                PregenerationConfig::from_config(app_config.get_pregeneration())
            };

            // Clone values before use to avoid borrow checker issues
            let database_path = app_config.database.path.clone();
            let image_directories = app_config.images.directories.clone();
            let cache_directory = app_config.get_cache_directory();
            let server_host = app_config.get_host();
            let server_port = app_config.get_port();

            // Use tokio runtime for async server
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(async {
                crate::server::run_server(
                    database_path,
                    image_directories,
                    static_dir,
                    cache_directory,
                    server_host,
                    server_port,
                    pregeneration_config,
                )
                .await
            })?;
        }
    }

    Ok(())
}
