use crate::db::Database;
use crate::models::GradingStatus;
use crate::utils::extract_filename;
use anyhow::Result;
use rusqlite::Connection;

pub fn show_images(conn: &Connection, ids_str: &str) -> Result<()> {
    let db = Database::new(conn);

    // Parse comma-separated IDs
    let ids: Vec<i32> = ids_str
        .split(',')
        .map(|s| s.trim().parse::<i32>())
        .collect::<Result<Vec<_>, _>>()?;

    if ids.is_empty() {
        println!("No image IDs provided.");
        return Ok(());
    }

    // Get images by IDs
    let images = db.get_images_by_ids(&ids)?;

    if images.is_empty() {
        println!("No images found with the provided IDs.");
        return Ok(());
    }

    // Display detailed information
    for image in &images {
        println!("\n{:-<60}", "");
        println!("Image ID: {}", image.id);
        println!("Project ID: {}", image.project_id);
        println!("Target ID: {}", image.target_id);

        if let Some(date) = image.acquired_date
            && let Some(dt) = chrono::DateTime::from_timestamp(date, 0)
        {
            println!("Acquired Date: {}", dt.format("%Y-%m-%d %H:%M:%S"));
        }

        println!("Filter: {}", image.filter_name);
        println!(
            "Status: {} ({})",
            GradingStatus::from_i32(image.grading_status),
            image.grading_status
        );

        if let Some(reason) = &image.reject_reason {
            println!("Reject Reason: {}", reason);
        }

        if let Some(filename) = extract_filename(&image.metadata) {
            println!("Filename: {}", filename);
        }

        // Parse and display metadata
        if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
            println!("\nMetadata:");
            if let Some(hfr) = metadata["HFR"].as_f64() {
                println!("  HFR: {:.3}", hfr);
            }
            if let Some(stars) = metadata["DetectedStars"].as_i64() {
                println!("  Detected Stars: {}", stars);
            }
            if let Some(target) = metadata["TargetName"].as_str() {
                println!("  Target Name: {}", target);
            }
            if let Some(exposure) = metadata["ExposureTime"].as_f64() {
                println!("  Exposure Time: {:.1}s", exposure);
            }
            if let Some(gain) = metadata["Gain"].as_i64() {
                println!("  Gain: {}", gain);
            }
            if let Some(offset) = metadata["Offset"].as_i64() {
                println!("  Offset: {}", offset);
            }
        }
    }

    println!("\n{:-<60}", "");
    println!("Total images found: {}", images.len());

    Ok(())
}
