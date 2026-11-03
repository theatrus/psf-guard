use crate::directory_tree::DirectoryTree;
use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::{FitsImage, ImageStatistics as ComputedStats};
use crate::nina_star_detection::{
    detect_stars_with_original, NoiseReduction, StarDetectionParams, StarSensitivity,
};
use crate::psf_fitting::PSFType;
use anyhow::Result;
use rusqlite::Connection;
use seiza_stretch::{stretch_u16_to_u16, StretchParams};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct ImageMetadata {
    #[serde(rename = "FileName")]
    filename: Option<String>,
    #[serde(rename = "HFR")]
    hfr: Option<f64>,
    #[serde(rename = "DetectedStars")]
    detected_stars: Option<i32>,
}

#[derive(Debug, Clone)]
struct DetectorConfig {
    name: String,
    detector: String,
    sensitivity: String,
}

#[allow(clippy::too_many_arguments)]
pub fn analyze_fits_and_compare(
    conn: &Connection,
    fits_path: &str,
    _project_filter: Option<String>,
    _target_filter: Option<String>,
    format: &str,
    detector: &str,
    sensitivity: &str,
    apply_stretch: bool,
    compare_all: bool,
    psf_type: &str,
    _verbose: bool,
) -> Result<()> {
    let fits_path = Path::new(fits_path);

    if compare_all {
        // Generate all combinations of detector configurations
        let configs = generate_detector_configs();

        if fits_path.is_file() {
            compare_single_fits_all_detectors(conn, fits_path, format, apply_stretch, &configs)?;
        } else if fits_path.is_dir() {
            println!("Comparison mode for directories not yet implemented");
            return Ok(());
        }
    } else {
        // Single detector mode
        if fits_path.is_file() {
            analyze_single_fits(
                conn,
                fits_path,
                format,
                detector,
                sensitivity,
                apply_stretch,
                psf_type,
            )?;
        } else if fits_path.is_dir() {
            analyze_fits_directory(
                conn,
                fits_path,
                format,
                detector,
                sensitivity,
                apply_stretch,
                psf_type,
            )?;
        } else {
            return Err(anyhow::anyhow!(
                "Path does not exist or is not accessible: {}",
                fits_path.display()
            ));
        }
    }

    Ok(())
}

fn generate_detector_configs() -> Vec<DetectorConfig> {
    let mut configs = vec![];

    // NINA detector configurations
    for sensitivity in &["normal", "high", "highest"] {
        configs.push(DetectorConfig {
            name: format!("NINA-{}", sensitivity),
            detector: "nina".to_string(),
            sensitivity: sensitivity.to_string(),
        });
    }

    // HocusFocus configuration
    configs.push(DetectorConfig {
        name: "HocusFocus".to_string(),
        detector: "hocusfocus".to_string(),
        sensitivity: "normal".to_string(),
    });

    configs
}

fn compare_single_fits_all_detectors(
    conn: &Connection,
    fits_path: &Path,
    format: &str,
    apply_stretch: bool,
    configs: &[DetectorConfig],
) -> Result<()> {
    let filename = fits_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    println!("Analyzing FITS file: {}", fits_path.display());

    // Load the FITS file once
    let fits = FitsImage::from_file(fits_path)?;
    let computed_stats = fits.calculate_basic_statistics();

    // Get database info if available
    let db_info = get_database_info(conn, filename)?;

    match format {
        "csv" => {
            println!("Detector,Stars,AvgHFR,HFRStdDev");
        }
        "json" => {
            let mut results = vec![];
            for config in configs {
                let result = run_detector_config(&fits, &computed_stats, config, apply_stretch);
                if let Ok((star_count, avg_hfr, hfr_std)) = result {
                    results.push(serde_json::json!({
                        "detector": config.name,
                        "stars": star_count,
                        "avg_hfr": avg_hfr,
                        "hfr_std_dev": hfr_std,
                    }));
                }
            }
            let output = serde_json::json!({
                "file": filename,
                "dimensions": format!("{}x{}", fits.width, fits.height),
                "statistics": {
                    "min": computed_stats.min,
                    "max": computed_stats.max,
                    "mean": computed_stats.mean,
                    "median": computed_stats.median,
                    "mad": computed_stats.mad.unwrap_or(0.0),
                },
                "database": db_info,
                "detectors": results,
            });
            println!("{}", serde_json::to_string_pretty(&output)?);
            return Ok(());
        }
        _ => {
            println!("\n=== Detector Comparison Results ===\n");
            println!("File: {}", filename);
            println!("Dimensions: {}x{}", fits.width, fits.height);
            println!(
                "Statistics: Min={}, Max={}, Mean={:.2}, Median={:.2}, MAD={:.2}",
                computed_stats.min,
                computed_stats.max,
                computed_stats.mean,
                computed_stats.median,
                computed_stats.mad.unwrap_or(0.0)
            );

            if let Some((nina_stars, nina_hfr)) = db_info {
                println!(
                    "N.I.N.A. Database: {} stars, HFR={:.3}",
                    nina_stars, nina_hfr
                );
            }

            println!(
                "\n{:<30} | {:>8} | {:>10} | {:>10}",
                "Detector", "Stars", "Avg HFR", "HFR StdDev"
            );
            println!("{:-<30}-+-{:-<8}-+-{:-<10}-+-{:-<10}", "", "", "", "");
        }
    }

    // Run each detector configuration
    for config in configs {
        let result = run_detector_config(&fits, &computed_stats, config, apply_stretch);

        match format {
            "csv" => {
                if let Ok((star_count, avg_hfr, hfr_std)) = result {
                    println!(
                        "{},{},{:.3},{:.3}",
                        config.name, star_count, avg_hfr, hfr_std
                    );
                }
            }
            _ => match result {
                Ok((star_count, avg_hfr, hfr_std)) => {
                    println!(
                        "{:<30} | {:>8} | {:>10.3} | {:>10.3}",
                        config.name, star_count, avg_hfr, hfr_std
                    );
                }
                Err(e) => {
                    println!("{:<30} | ERROR: {}", config.name, e);
                }
            },
        }
    }

    Ok(())
}

fn run_detector_config(
    fits: &FitsImage,
    computed_stats: &ComputedStats,
    config: &DetectorConfig,
    apply_stretch: bool,
) -> Result<(usize, f64, f64)> {
    match config.detector.as_str() {
        "nina" => {
            let star_sensitivity = match config.sensitivity.as_str() {
                "high" => StarSensitivity::High,
                "highest" => StarSensitivity::Highest,
                _ => StarSensitivity::Normal,
            };

            let params = StarDetectionParams {
                sensitivity: star_sensitivity,
                noise_reduction: NoiseReduction::None,
                use_roi: false,
                ..StarDetectionParams::default()
            };

            // NINA always uses MTF stretch
            let stretch_params = StretchParams::default();
            let stretched = stretch_u16_to_u16(
                &fits.data,
                &computed_stats.to_stretch_statistics(),
                &stretch_params,
            );

            let result = detect_stars_with_original(
                &stretched,
                &fits.data,
                fits.width,
                fits.height,
                &params,
            );

            Ok((
                result.star_list.len(),
                result.average_hfr,
                result.hfr_std_dev,
            ))
        }
        "hocusfocus" => {
            let params = HocusFocusParams::default();

            let detection_data = if apply_stretch {
                let stretch_params = StretchParams::default();
                stretch_u16_to_u16(
                    &fits.data,
                    &computed_stats.to_stretch_statistics(),
                    &stretch_params,
                )
            } else {
                fits.data.clone()
            };

            let result =
                detect_stars_hocus_focus(&detection_data, fits.width, fits.height, &params);

            if result.stars.is_empty() {
                Ok((0, 0.0, 0.0))
            } else {
                let hfr_values: Vec<f64> = result.stars.iter().map(|s| s.hfr).collect();
                let avg_hfr = hfr_values.iter().sum::<f64>() / hfr_values.len() as f64;
                let variance = hfr_values
                    .iter()
                    .map(|&hfr| (hfr - avg_hfr).powi(2))
                    .sum::<f64>()
                    / hfr_values.len() as f64;
                let std_dev = variance.sqrt();

                Ok((result.stars.len(), avg_hfr, std_dev))
            }
        }
        _ => Err(anyhow::anyhow!("Unknown detector: {}", config.detector)),
    }
}

fn analyze_single_fits(
    conn: &Connection,
    fits_path: &Path,
    format: &str,
    detector: &str,
    sensitivity: &str,
    apply_stretch: bool,
    psf_type: &str,
) -> Result<()> {
    let filename = fits_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");

    println!("Analyzing FITS file: {}", fits_path.display());

    // Load the FITS file
    let fits = FitsImage::from_file(fits_path)?;
    let computed_stats = fits.calculate_basic_statistics();

    // Perform star detection
    let (star_count, avg_hfr, hfr_std, detection_info) = detect_stars(
        &fits,
        &computed_stats,
        detector,
        sensitivity,
        apply_stretch,
        psf_type,
    )?;

    // Look for matching database entries
    let db_info = get_database_info(conn, filename)?;

    // Output results based on format
    match format {
        "json" => output_json(
            &computed_stats,
            star_count,
            avg_hfr,
            hfr_std,
            db_info,
            &detection_info,
            filename,
        ),
        "csv" => output_csv(
            filename,
            &computed_stats,
            star_count,
            avg_hfr,
            hfr_std,
            db_info,
        ),
        _ => output_table(
            filename,
            &computed_stats,
            star_count,
            avg_hfr,
            hfr_std,
            db_info,
            &detection_info,
        ),
    }

    Ok(())
}

fn analyze_fits_directory(
    conn: &Connection,
    dir_path: &Path,
    format: &str,
    detector: &str,
    sensitivity: &str,
    apply_stretch: bool,
    psf_type: &str,
) -> Result<()> {
    // Build directory tree cache and get FITS files
    let directory_tree = DirectoryTree::build(dir_path)?;
    let fits_files: Vec<PathBuf> = directory_tree
        .get_fits_files()
        .into_iter()
        .cloned()
        .collect();

    if fits_files.is_empty() {
        println!("No FITS files found in directory: {}", dir_path.display());
        return Ok(());
    }

    println!("Found {} FITS files to analyze", fits_files.len());

    // CSV header for CSV format
    if format == "csv" {
        println!("Filename,Min,Max,Mean,Median,MAD,DetectedStars,AvgHFR,HFRStdDev,DBStars,DBHFR");
    }

    for fits_path in fits_files {
        if let Err(e) = analyze_single_fits(
            conn,
            &fits_path,
            format,
            detector,
            sensitivity,
            apply_stretch,
            psf_type,
        ) {
            eprintln!("Error analyzing {}: {}", fits_path.display(), e);
        }
    }

    Ok(())
}

fn detect_stars(
    fits: &FitsImage,
    computed_stats: &ComputedStats,
    detector: &str,
    sensitivity: &str,
    apply_stretch: bool,
    psf_type: &str,
) -> Result<(usize, f64, f64, String)> {
    let detection_info;

    println!("\nStar Detection:");
    println!("  Algorithm: {}", detector);
    println!("  Sensitivity: {}", sensitivity);
    println!("  Apply MTF Stretch: {}", apply_stretch);

    match detector.to_lowercase().as_str() {
        "nina" => {
            // Parse sensitivity
            let star_sensitivity = match sensitivity.to_lowercase().as_str() {
                "high" => StarSensitivity::High,
                "highest" => StarSensitivity::Highest,
                _ => StarSensitivity::Normal,
            };
            println!("  Forcing stretch for NINA");

            let params = StarDetectionParams {
                sensitivity: star_sensitivity,
                noise_reduction: NoiseReduction::None,
                use_roi: false,
                ..StarDetectionParams::default()
            };

            // NINA always uses MTF stretch
            let stretch_params = StretchParams::default();
            let stretched = stretch_u16_to_u16(
                &fits.data,
                &computed_stats.to_stretch_statistics(),
                &stretch_params,
            );

            let result = detect_stars_with_original(
                &stretched,
                &fits.data,
                fits.width,
                fits.height,
                &params,
            );

            detection_info = format!("NINA {} sensitivity", sensitivity);

            Ok((
                result.star_list.len(),
                result.average_hfr,
                result.hfr_std_dev,
                detection_info,
            ))
        }
        "hocusfocus" => {
            println!("  Using pure-Rust image processing (seiza-imgproc)");

            // Parse PSF type
            let params = HocusFocusParams {
                psf_type: psf_type.parse().unwrap_or(PSFType::None),
                ..Default::default()
            };
            if params.psf_type != PSFType::None {
                println!("  PSF Fitting: {:?}", params.psf_type);
            }

            let detection_data = if apply_stretch {
                let stretch_params = StretchParams::default();
                stretch_u16_to_u16(
                    &fits.data,
                    &computed_stats.to_stretch_statistics(),
                    &stretch_params,
                )
            } else {
                fits.data.clone()
            };

            let result =
                detect_stars_hocus_focus(&detection_data, fits.width, fits.height, &params);

            detection_info = "HocusFocus".to_string();

            if result.stars.is_empty() {
                Ok((0, 0.0, 0.0, detection_info))
            } else {
                // Calculate statistics
                let hfr_values: Vec<f64> = result.stars.iter().map(|s| s.hfr).collect();
                let avg_hfr = hfr_values.iter().sum::<f64>() / hfr_values.len() as f64;
                let variance = hfr_values
                    .iter()
                    .map(|&hfr| (hfr - avg_hfr).powi(2))
                    .sum::<f64>()
                    / hfr_values.len() as f64;
                let std_dev = variance.sqrt();

                Ok((result.stars.len(), avg_hfr, std_dev, detection_info))
            }
        }
        _ => Err(anyhow::anyhow!("Unknown detector: {}", detector)),
    }
}

fn get_database_info(conn: &Connection, filename: &str) -> Result<Option<(i32, f64)>> {
    // Simple query to find images by filename pattern
    let query = "SELECT metadata FROM acquiredimage WHERE metadata LIKE ?";
    let pattern = format!("%{}%", filename);

    let mut stmt = conn.prepare(query)?;
    let mut rows = stmt.query([&pattern])?;

    while let Some(row) = rows.next()? {
        let metadata_json: String = row.get(0)?;

        // Try to parse the metadata
        if let Ok(metadata) = serde_json::from_str::<ImageMetadata>(&metadata_json) {
            // Check if filename matches
            if let Some(meta_filename) = metadata.filename
                && (meta_filename.contains(filename) || filename.contains(&meta_filename))
                && let (Some(stars), Some(hfr)) = (metadata.detected_stars, metadata.hfr)
            {
                return Ok(Some((stars, hfr)));
            }
        }
    }

    Ok(None)
}

fn output_table(
    filename: &str,
    computed_stats: &ComputedStats,
    star_count: usize,
    avg_hfr: f64,
    hfr_std: f64,
    db_info: Option<(i32, f64)>,
    detection_info: &str,
) {
    println!("\n=== FITS Analysis Results ===");
    println!("File: {}", filename);
    println!("\nImage Statistics:");
    println!("  Min: {}", computed_stats.min);
    println!("  Max: {}", computed_stats.max);
    println!("  Mean: {:.2}", computed_stats.mean);
    println!("  Median: {:.2}", computed_stats.median);
    println!("  MAD: {:.2}", computed_stats.mad.unwrap_or(0.0));

    println!("\nDetection Results ({}):", detection_info);
    println!("  Detected Stars: {}", star_count);
    println!("  Average HFR: {:.3}", avg_hfr);
    println!("  HFR Std Dev: {:.3}", hfr_std);

    if let Some((nina_stars, nina_hfr)) = db_info {
        println!("\nDatabase Comparison:");
        println!("  N.I.N.A. Stars: {}", nina_stars);
        println!("  N.I.N.A. HFR: {:.3}", nina_hfr);

        let star_diff = (star_count as f64 - nina_stars as f64) / nina_stars as f64 * 100.0;
        let hfr_diff = (avg_hfr - nina_hfr) / nina_hfr * 100.0;

        println!("  Star Count Difference: {:.1}%", star_diff);
        println!("  HFR Difference: {:.1}%", hfr_diff);
    } else {
        println!("\nNo matching database entry found");
    }
}

fn output_json(
    computed_stats: &ComputedStats,
    star_count: usize,
    avg_hfr: f64,
    hfr_std: f64,
    db_info: Option<(i32, f64)>,
    detection_info: &str,
    filename: &str,
) {
    let result = serde_json::json!({
        "file": filename,
        "computed": {
            "statistics": {
                "min": computed_stats.min,
                "max": computed_stats.max,
                "mean": computed_stats.mean,
                "median": computed_stats.median,
                "mad": computed_stats.mad.unwrap_or(0.0),
            },
            "detection": {
                "algorithm": detection_info,
                "stars": star_count,
                "average_hfr": avg_hfr,
                "hfr_std_dev": hfr_std,
            }
        },
        "database": db_info.map(|(stars, hfr)| {
            serde_json::json!({
                "stars": stars,
                "hfr": hfr,
            })
        }),
    });

    println!("{}", serde_json::to_string_pretty(&result).unwrap());
}

fn output_csv(
    filename: &str,
    computed_stats: &ComputedStats,
    star_count: usize,
    avg_hfr: f64,
    hfr_std: f64,
    db_info: Option<(i32, f64)>,
) {
    let (db_stars, db_hfr) = db_info.unwrap_or((0, 0.0));

    println!(
        "{},{},{},{:.2},{:.2},{:.2},{},{:.3},{:.3},{},{:.3}",
        filename,
        computed_stats.min,
        computed_stats.max,
        computed_stats.mean,
        computed_stats.median,
        computed_stats.mad.unwrap_or(0.0),
        star_count,
        avg_hfr,
        hfr_std,
        db_stars,
        db_hfr
    );
}

/// A rig's star detection from `analyze-fits --scan`'s options, by the
/// names the registry stores.
pub fn scan_settings(
    detector: &str,
    sensitivity: &str,
    noise_reduction: &str,
    hocus_focus_version: &str,
) -> Result<crate::star_detection::RigStarDetection> {
    fn choice<T: serde::de::DeserializeOwned>(option: &str, value: &str) -> Result<T> {
        serde_json::from_value(serde_json::Value::String(value.to_ascii_lowercase()))
            .map_err(|_| anyhow::anyhow!("--{option} does not take {value:?}"))
    }
    Ok(crate::star_detection::RigStarDetection {
        detector: choice("scan-detector", detector)?,
        sensitivity: choice("scan-sensitivity", sensitivity)?,
        noise_reduction: choice("scan-noise-reduction", noise_reduction)?,
        hocus_focus_version: choice("hocus-focus-version", hocus_focus_version)?,
    })
}

/// Every frame at `path`: the file itself, or each one below the folder.
fn frames_under(path: &Path) -> Vec<PathBuf> {
    if path.is_file() {
        return vec![path.to_path_buf()];
    }
    let mut frames = Vec::new();
    let mut folders = vec![path.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&folder) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if crate::image_io::is_image_path(&path) {
                frames.push(path);
            }
        }
    }
    frames.sort();
    frames
}

/// Measure each frame at `path` as the quality scan does with `rig`'s star
/// detection, and print a JSON line per frame with the catalog row's star
/// count, HFR and FWHM beside the measurement. Rows match by file name.
pub fn analyze_as_scan(
    conn: &Connection,
    path: &str,
    settings: &crate::star_detection::StarDetectionSettings,
) -> Result<()> {
    struct Row {
        metadata: String,
        profile_id: Option<String>,
        captured_at: Option<i64>,
    }
    let mut by_name: std::collections::HashMap<String, Row> = Default::default();
    let mut statement =
        conn.prepare("SELECT metadata, profileId, acquireddate FROM acquiredimage")?;
    let rows = statement.query_map([], |row| {
        Ok(Row {
            metadata: row.get(0)?,
            profile_id: row.get(1)?,
            captured_at: row.get(2)?,
        })
    })?;
    for row in rows {
        let row = row?;
        let metadata = &row.metadata;
        let name = serde_json::from_str::<serde_json::Value>(metadata)
            .ok()
            .and_then(|value| value["FileName"].as_str().map(str::to_string))
            .and_then(|name| name.split(['\\', '/']).next_back().map(str::to_string));
        if let Some(name) = name {
            by_name.insert(name, row);
        }
    }
    for frame in frames_under(Path::new(path)) {
        let name = frame
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let row = by_name.get(&name);
        let metadata = row.map_or("{}", |row| row.metadata.as_str());
        let catalog: serde_json::Value = serde_json::from_str(metadata).unwrap_or_default();
        let meta = crate::commands::import::headers::read_frame_meta(&frame);
        let facts = crate::star_detection::FrameFacts::of_frame(
            &meta,
            row.and_then(|row| row.profile_id.as_deref()),
            row.and_then(|row| row.captured_at),
        );
        let detection = settings.resolve(
            &facts,
            crate::star_detection::FwhmEvidence::of_metadata(metadata),
        );
        let started = std::time::Instant::now();
        let measured = FitsImage::from_file(&frame).map(|fits| {
            let stats = fits.calculate_basic_statistics();
            crate::server::spatial_scan::measure_frame_stars(
                &fits,
                &stats,
                &detection,
                meta.image_scale_arcsec,
            )
        });
        let line = match measured {
            Ok(stars) => serde_json::json!({
                "file": frame.display().to_string(),
                "software": facts.software,
                "rig_uuid": facts.rig_uuid,
                "image_scale_arcsec": meta.image_scale_arcsec,
                "detection": detection,
                "detector": stars.detector,
                "stars": stars.star_count,
                "hfr": stars.avg_hfr,
                "catalog_row": by_name.contains_key(&name),
                "nina_stars": catalog["DetectedStars"],
                "nina_hfr": catalog["HFR"],
                "nina_fwhm": catalog["FWHM"],
                "seconds": started.elapsed().as_secs_f64(),
            }),
            Err(error) => serde_json::json!({
                "file": frame.display().to_string(),
                "error": error.to_string(),
            }),
        };
        println!("{line}");
    }
    Ok(())
}
