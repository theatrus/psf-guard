use crate::db::Database;
use crate::grading;
use crate::models::GradingStatus;
use anyhow::Result;
use rusqlite::Connection;

pub fn regrade_images(
    conn: &Connection,
    dry_run: bool,
    target_filter: Option<String>,
    project_filter: Option<String>,
    days: u32,
    reset_mode: &str,
    stat_config: Option<grading::StatisticalGradingConfig>,
) -> Result<()> {
    // Validate reset mode
    match reset_mode {
        "none" | "automatic" | "all" => {}
        _ => {
            return Err(anyhow::anyhow!(
                "Invalid reset mode: {}. Use 'none', 'automatic', or 'all'",
                reset_mode
            ))
        }
    }

    let db = Database::new(conn);

    println!(
        "{}Regrading images...",
        if dry_run { "[DRY RUN] " } else { "" }
    );

    // Calculate date cutoff
    let now = chrono::Utc::now();
    let cutoff_date = now - chrono::Duration::days(days as i64);
    let cutoff_timestamp = cutoff_date.timestamp();

    println!("  Date range: {} to now", cutoff_date.format("%Y-%m-%d"));

    // Wrap all operations in a transaction for consistency
    if !dry_run && (reset_mode != "none" || stat_config.is_some()) {
        db.with_transaction(|_tx| {
            // First, handle reset if requested
            if reset_mode != "none" {
                handle_reset(
                    &db,
                    false, // Not a dry run inside transaction
                    reset_mode,
                    cutoff_timestamp,
                    &project_filter,
                    &target_filter,
                )?;
            }

            // Now perform statistical grading if enabled
            if let Some(config) = stat_config {
                perform_statistical_grading(
                    &db,
                    false, // Not a dry run inside transaction
                    cutoff_timestamp,
                    &project_filter,
                    &target_filter,
                    config,
                )?;
            }

            Ok(())
        })?;
    } else if dry_run {
        // For dry runs, execute without transaction
        if reset_mode != "none" {
            handle_reset(
                &db,
                dry_run,
                reset_mode,
                cutoff_timestamp,
                &project_filter,
                &target_filter,
            )?;
        }

        if let Some(config) = stat_config {
            perform_statistical_grading(
                &db,
                dry_run,
                cutoff_timestamp,
                &project_filter,
                &target_filter,
                config,
            )?;
        }
    }

    println!("\nRegrading complete.");

    if dry_run {
        println!("\nThis was a dry run. Use without --dry-run to actually update the database.");
    }

    Ok(())
}

fn handle_reset(
    db: &Database,
    dry_run: bool,
    reset_mode: &str,
    cutoff_timestamp: i64,
    project_filter: &Option<String>,
    target_filter: &Option<String>,
) -> Result<()> {
    println!("  Reset mode: {}", reset_mode);

    if dry_run {
        let count = db.count_images_to_reset(
            reset_mode,
            cutoff_timestamp,
            project_filter.as_deref(),
            target_filter.as_deref(),
        )?;
        println!("  Would reset {} images to pending status", count);
    } else {
        let affected = db.reset_grading_status(
            reset_mode,
            cutoff_timestamp,
            project_filter.as_deref(),
            target_filter.as_deref(),
        )?;
        println!("  Reset {} images to pending status", affected);
    }

    Ok(())
}

fn perform_statistical_grading(
    db: &Database,
    dry_run: bool,
    cutoff_timestamp: i64,
    project_filter: &Option<String>,
    target_filter: &Option<String>,
    config: grading::StatisticalGradingConfig,
) -> Result<()> {
    println!("\nPerforming statistical analysis...");

    // Get all images in date range
    let all_images = db.query_images(
        None,
        project_filter.as_deref(),
        target_filter.as_deref(),
        Some(cutoff_timestamp),
    )?;

    println!("  Analyzing {} images", all_images.len());

    // Convert to format expected by grader
    let mut image_stats = Vec::new();
    for (image, _project_name, target_name) in &all_images {
        match grading::light_statistics(image, target_name) {
            Ok(stats) => image_stats.push(stats),
            Err(e) => println!(
                "  Warning: Failed to parse metadata for image {}: {}",
                image.id, e
            ),
        }
    }

    // Run statistical analysis
    let grader = grading::StatisticalGrader::new(config);
    match grader.analyze_images(image_stats) {
        Ok(rejections) => {
            println!("  Found {} statistical rejections", rejections.len());

            if dry_run {
                // Just show what would be rejected
                for rejection in &rejections {
                    println!(
                        "    Would reject image {}: {} - {}",
                        rejection.image_id, rejection.reason, rejection.details
                    );
                }
            } else {
                // Build updates list
                let updates: Vec<(i32, GradingStatus, Option<String>)> = rejections
                    .iter()
                    .map(|r| {
                        (
                            r.image_id,
                            GradingStatus::Rejected,
                            Some(format!("[Auto] {} - {}", r.reason, r.details)),
                        )
                    })
                    .collect();

                // Apply updates
                db.batch_update_grading_status(&updates)?;
                println!("  Applied {} rejections", updates.len());
            }
        }
        Err(e) => println!("  Warning: Statistical analysis failed: {}", e),
    }

    Ok(())
}
