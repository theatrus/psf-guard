//! Screen a directory of FITS light frames for occlusion, clouds and stray
//! light without needing a scheduler database. With `--regrade-db`, fresh
//! pixel-derived plate solutions also contribute off-target, pointing-jump,
//! tracking-drift, and corroborated no-solve evidence to the shared sequence
//! grader before any writes are proposed.
//!
//! Per frame: star detection (positions included), grid-based spatial metrics
//! (dead-cell fraction, background spread) and image statistics in physical
//! ADU. Frames are grouped by (filter, exposure) from FITS headers, ordered
//! by DATE-OBS, and run through the `SequenceAnalyzer` so both absolute
//! (spatial) and sequence-relative (temporal) signals contribute. Prints a
//! per-frame verdict: OK / WARN / REJECT.

use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
use crate::image_analysis::FitsImage;
use crate::mtf_stretch::{stretch_image, StretchParameters};
use crate::nina_star_detection::{
    detect_stars_with_original, NoiseReduction, StarDetectionParams, StarSensitivity,
};
use crate::photometry::{
    sequence_screening_signals, split_sessions, CatalogStar, FrameCatalog, FrameInputs,
    PhotometryConfig,
};
use crate::sequence_analysis::{
    AstrometryFrameMetrics, ImageMetrics, IssueCategory, SequenceAnalyzer, SequenceAnalyzerConfig,
};
use crate::spatial_analysis::{compute_spatial_metrics, PixelCalibration, SpatialAnalysisConfig};
use anyhow::Result;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Maximum |DATE-OBS - acquireddate| for matching a raw FITS file to a TS row.
const MATCH_TOLERANCE_SECS: i64 = 600;
type RegradeAstrometryMatch = (i32, Option<i64>, Option<(f64, f64)>);

#[derive(Debug, Clone)]
pub struct ScreenOptions {
    pub detector: String,
    pub format: String,
    pub min_score: f64,
    pub dead_cell_rise: f64,
    pub threads: Option<usize>,
    pub session_gap_minutes: u64,
    /// Registry slug or path of a scheduler DB to write `[Auto]` rejections
    /// into for frames with a REJECT verdict (matched by FITS basename and
    /// capture time). This also enables fresh astrometry quality analysis.
    pub regrade_db: Option<String>,
    /// Report what the regrade would change without writing.
    pub dry_run: bool,
    /// Registry override for resolving `regrade_db` slugs.
    pub registry: Option<String>,
    /// Directory to write annotated diagnostic PNGs for WARN/REJECT frames.
    pub annotate_dir: Option<String>,
}

#[derive(Debug, Clone)]
struct FrameRecord {
    path: PathBuf,
    filter: String,
    exposure_s: Option<f64>,
    timestamp: Option<i64>,
    star_count: usize,
    avg_hfr: f64,
    median_adu: f64,
    dead_cell_fraction: Option<f64>,
    star_uniformity: Option<f64>,
    bg_cell_spread: f64,
    bg_cell_max_dev: f64,
    width: usize,
    height: usize,
    /// Star positions + ADU fluxes for cross-frame photometry (empty for
    /// detectors without flux measurements).
    catalog: FrameCatalog,
    star_cell_counts: Vec<f64>,
    bg_cell_medians: Vec<f64>,
    bg_glow_max: f64,
    bg_glow_cells: Vec<bool>,
    astrometry: Option<AstrometryFrameMetrics>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "UPPERCASE")]
enum Verdict {
    Ok,
    Warn,
    Reject,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Verdict::Ok => write!(f, "OK"),
            Verdict::Warn => write!(f, "WARN"),
            Verdict::Reject => write!(f, "REJECT"),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
struct ScreenResult {
    /// Index into the analysis records (for annotation); not serialized.
    #[serde(skip)]
    record_idx: usize,
    file: String,
    filter: String,
    exposure_s: Option<f64>,
    timestamp: Option<i64>,
    star_count: usize,
    avg_hfr: f64,
    median_adu: f64,
    dead_cell_fraction: Option<f64>,
    star_uniformity: Option<f64>,
    bg_cell_spread: f64,
    bg_cell_max_dev: f64,
    transparency: Option<f64>,
    extinction_cell_fraction: Option<f64>,
    star_cell_drop_fraction: Option<f64>,
    bg_cell_rise_fraction: Option<f64>,
    bg_cell_fall_fraction: Option<f64>,
    bg_glow_max: Option<f64>,
    quality_score: Option<f64>,
    category: Option<IssueCategory>,
    flags: Vec<IssueCategory>,
    pointing: Option<crate::sequence_analysis::PointingQuality>,
    regrade_reason: Option<String>,
    details: Option<String>,
    verdict: Verdict,
}

pub fn screen_fits(path: &str, options: &ScreenOptions) -> Result<()> {
    let dir = Path::new(path);
    let files = collect_fits_files(dir)?;
    if files.is_empty() {
        return Err(anyhow::anyhow!(
            "No FITS files found under: {}",
            dir.display()
        ));
    }
    eprintln!("Screening {} FITS frames...", files.len());

    let mut records = analyze_frames(&files, options)?;
    // Deterministic ordering regardless of worker completion order: sort by
    // (timestamp, filename). Frames without a parseable DATE-OBS tie on
    // timestamp and fall back to filename order (N.I.N.A. names sort
    // chronologically), so sequence baselines - and therefore verdicts - are
    // identical run to run. The analyzer's internal sort is stable, so this
    // order survives into scoring.
    records.sort_by(|a, b| {
        (a.timestamp.unwrap_or(0), a.path.as_path())
            .cmp(&(b.timestamp.unwrap_or(0), b.path.as_path()))
    });
    if options.regrade_db.is_some() {
        enrich_astrometry_for_regrade(&mut records, options)?;
    }
    let (results, signals_by_idx) = score_records(&records, options);

    // Sort for output: filter, then timestamp.
    let mut results = results;
    results.sort_by(|a, b| {
        (a.filter.as_str(), a.timestamp.unwrap_or(0), a.file.as_str()).cmp(&(
            b.filter.as_str(),
            b.timestamp.unwrap_or(0),
            b.file.as_str(),
        ))
    });

    match options.format.as_str() {
        "json" => print_json(&results)?,
        "csv" => print_csv(&results),
        _ => print_table(&results),
    }

    if let Some(dir) = &options.annotate_dir {
        annotate_flagged(&results, &records, &signals_by_idx, dir)?;
    }

    if let Some(db_arg) = &options.regrade_db {
        apply_regrade(&results, db_arg, options)?;
    }

    Ok(())
}

/// Render diagnostic PNGs for every WARN/REJECT frame into `dir`, showing
/// which grid cells drove the verdict (see `screen_annotate`).
fn annotate_flagged(
    results: &[ScreenResult],
    records: &[FrameRecord],
    signals: &HashMap<usize, crate::photometry::FrameSignals>,
    dir: &str,
) -> Result<()> {
    use crate::commands::screen_annotate::{render_annotated_frame, AnnotationData};

    let out_dir = Path::new(dir);
    std::fs::create_dir_all(out_dir)?;
    let spatial_config = SpatialAnalysisConfig::default();

    let flagged: Vec<&ScreenResult> = results
        .iter()
        .filter(|r| r.verdict != Verdict::Ok)
        .collect();
    eprintln!(
        "Annotating {} flagged frames into {}...",
        flagged.len(),
        out_dir.display()
    );

    for r in flagged {
        let record = &records[r.record_idx];
        let sig = signals.get(&r.record_idx);

        let mut caption = vec![
            format!(
                "{} {} SCORE={}",
                r.verdict,
                category_label(&r.category).to_uppercase(),
                r.quality_score
                    .map(|s| format!("{:.2}", s))
                    .unwrap_or_else(|| "-".into()),
            ),
            format!(
                "STARS={} HFR={:.2} DEAD={} TRANSP={} EXT={} DROP={} BGRISE={}",
                r.star_count,
                r.avg_hfr,
                r.dead_cell_fraction
                    .map(|v| format!("{:.0}%", v * 100.0))
                    .unwrap_or_else(|| "-".into()),
                r.transparency
                    .map(|v| format!("{:.2}", v))
                    .unwrap_or_else(|| "-".into()),
                r.extinction_cell_fraction
                    .map(|v| format!("{:.0}%", v * 100.0))
                    .unwrap_or_else(|| "-".into()),
                r.star_cell_drop_fraction
                    .map(|v| format!("{:.0}%", v * 100.0))
                    .unwrap_or_else(|| "-".into()),
                r.bg_cell_rise_fraction
                    .map(|v| format!("{:.0}%", v * 100.0))
                    .unwrap_or_else(|| "-".into()),
            ),
        ];
        if let Some(fall) = r.bg_cell_fall_fraction.filter(|&v| v > 0.0)
            && let Some(first) = caption.get_mut(1)
        {
            first.push_str(&format!(" FALL={:.0}%", fall * 100.0));
        }
        if let Some(glow) = r.bg_glow_max.filter(|&v| v > 0.0)
            && let Some(first) = caption.get_mut(1)
        {
            first.push_str(&format!(" GLOW={:.1}%", glow * 100.0));
        }
        if let Some(details) = &r.details {
            caption.push(details.chars().take(110).collect());
        }

        let data = AnnotationData {
            grid_cols: spatial_config.grid_cols,
            grid_rows: spatial_config.grid_rows,
            star_cell_counts: record.star_cell_counts.clone(),
            cell_relative_ratios: sig
                .map(|s| s.cell_relative_ratios.clone())
                .unwrap_or_default(),
            star_drop_cells: sig.map(|s| s.star_drop_cells.clone()).unwrap_or_default(),
            bg_rise_cells: sig.map(|s| s.bg_rise_cells.clone()).unwrap_or_default(),
            bg_fall_cells: sig.map(|s| s.bg_fall_cells.clone()).unwrap_or_default(),
            bg_glow_cells: record.bg_glow_cells.clone(),
            caption_lines: caption,
        };

        let fits = match FitsImage::from_file(&record.path) {
            Ok(f) => f,
            Err(e) => {
                eprintln!("  Skipping annotation for {}: {}", r.file, e);
                continue;
            }
        };
        let out_path = out_dir.join(format!(
            "{}.{}.png",
            record
                .path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("frame"),
            r.verdict.to_string().to_lowercase()
        ));
        if let Err(e) = render_annotated_frame(&fits, &data, &out_path) {
            eprintln!("  Failed to annotate {}: {}", r.file, e);
        }
    }
    Ok(())
}

/// When `--regrade-db` is active, solve the screened files against the
/// intended Target Scheduler target before scoring. The same conservative
/// basename + timestamp match used by the write-back path prevents an
/// unrelated directory from acquiring another target's coordinates.
fn enrich_astrometry_for_regrade(
    records: &mut [FrameRecord],
    options: &ScreenOptions,
) -> Result<()> {
    use crate::commands::sync::resolve_db_path;
    use crate::db::Database;
    use crate::db_registry::DbRegistry;
    use rusqlite::Connection;

    let Some(db_arg) = options.regrade_db.as_deref() else {
        return Ok(());
    };
    let registry = match &options.registry {
        Some(path) => DbRegistry::load_or_init(Path::new(path)).ok(),
        None => DbRegistry::default_path()
            .ok()
            .and_then(|path| DbRegistry::load_or_init(&path).ok()),
    };
    let db_path = resolve_db_path(registry.as_ref(), db_arg)?;
    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let db = Database::new(&conn);
    let mut resolver = crate::acquisition_context::FramingResolver::new(&conn)?;
    let mut by_basename: HashMap<String, Vec<RegradeAstrometryMatch>> = HashMap::new();
    for (image, _, _) in db.query_images(None, None, None, None)? {
        let Some(filename) = serde_json::from_str::<serde_json::Value>(&image.metadata)
            .ok()
            .and_then(|metadata| metadata["FileName"].as_str().map(str::to_string))
        else {
            continue;
        };
        let Some(basename) = filename.split(&['\\', '/'][..]).next_back() else {
            continue;
        };
        let expected = resolver.expected_for_grading(&conn, &image)?;
        by_basename.entry(basename.to_string()).or_default().push((
            image.id,
            image.acquired_date,
            expected,
        ));
    }

    let astrometry = crate::astrometry::AstrometryContext::new(
        registry
            .as_ref()
            .and_then(|registry| registry.astrometry.clone())
            .unwrap_or_default(),
    );
    // Match first so the progress denominator reflects real solve work, then
    // solve the unambiguous matches serially (solving is memory-heavy).
    type MatchedRecord = (usize, i32, Option<(f64, f64)>);
    let matched: Vec<MatchedRecord> = records
        .iter()
        .enumerate()
        .filter_map(|(idx, record)| {
            let file = record.path.file_name()?.to_str()?;
            let [(image_id, acquired, expected)] = by_basename.get(file).map(Vec::as_slice)?
            else {
                return None;
            };
            matches!((record.timestamp, *acquired), (Some(ours), Some(theirs)) if (ours - theirs).abs() <= MATCH_TOLERANCE_SECS)
                .then_some((idx, *image_id, *expected))
        })
        .collect();
    let total_matched = matched.len();
    for (attempted, (idx, image_id, expected)) in matched.into_iter().enumerate() {
        let record = &mut records[idx];
        let file = record
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        eprintln!("[astrometry {}/{}] {}", attempted + 1, total_matched, file);
        match astrometry.solve_image_for_quality(image_id, &record.path, expected) {
            Ok(analysis) => {
                record.astrometry =
                    crate::sequence_analysis::astrometry_metrics_from_analysis(&analysis);
            }
            Err(error) => eprintln!("  Astrometry unavailable for {file}: {error}"),
        }
    }
    Ok(())
}

/// Write `[Auto]` rejections for REJECT verdicts into a scheduler DB,
/// matching frames by FITS basename. Frames already rejected in the DB are
/// left untouched (manual and prior auto rejections are preserved); Pending
/// and Accepted frames are regraded, since a wrongly Accepted occluded frame
/// is exactly the case this screening exists for.
fn apply_regrade(results: &[ScreenResult], db_arg: &str, options: &ScreenOptions) -> Result<()> {
    use crate::commands::sync::resolve_db_path;
    use crate::db::Database;
    use crate::db_registry::DbRegistry;
    use crate::models::GradingStatus;
    use rusqlite::Connection;

    let registry = match &options.registry {
        Some(p) => DbRegistry::load_or_init(Path::new(p)).ok(),
        None => DbRegistry::default_path()
            .ok()
            .and_then(|p| DbRegistry::load_or_init(&p).ok()),
    };
    let db_path = resolve_db_path(registry.as_ref(), db_arg)?;
    // READ_WRITE without CREATE: a stale registry path must error, not
    // silently create an empty database file (especially under --dry-run).
    let conn = Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    let db = Database::new(&conn);

    // Basename -> (image id, grading status, acquireddate), for every image
    // in the DB. N.I.N.A. filenames embed a timestamp, so basenames are
    // unique per capture; ambiguous matches are skipped defensively.
    let mut by_basename: HashMap<String, Vec<(i32, i32, Option<i64>)>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT Id, gradingStatus, acquireddate, json_extract(metadata, '$.FileName') \
             FROM acquiredimage",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i32>(0)?,
                row.get::<_, i32>(1)?,
                row.get::<_, Option<i64>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        for row in rows {
            let (id, status, acquired, filename) = row?;
            let Some(filename) = filename else { continue };
            let Some(base) = filename.split(&['\\', '/'][..]).next_back() else {
                continue;
            };
            by_basename
                .entry(base.to_string())
                .or_default()
                .push((id, status, acquired));
        }
    }

    // Maximum |DATE-OBS - acquireddate| for a match. Both are UTC epoch
    // seconds written by N.I.N.A. for the same capture (observed skew ~1s);
    // 10 minutes absorbs exposure-length/save-time differences while
    // rejecting same-basename rows from other sessions.
    let mut updates: Vec<(i32, GradingStatus, Option<String>)> = Vec::new();
    let mut unmatched = 0usize;
    let mut ambiguous = 0usize;
    let mut already_rejected = 0usize;

    for r in results.iter().filter(|r| r.verdict == Verdict::Reject) {
        let matches = by_basename.get(&r.file);
        let (id, status) = match matches.map(|m| m.as_slice()) {
            Some([single]) => {
                // Cross-check capture time so a basename collision with a
                // different session's image can never regrade the wrong row.
                match (r.timestamp, single.2) {
                    (Some(ours), Some(theirs)) if (ours - theirs).abs() <= MATCH_TOLERANCE_SECS => {
                        (single.0, single.1)
                    }
                    _ => {
                        eprintln!(
                            "  Timestamp mismatch (or missing), skipping: {} (screened {:?}, db {:?})",
                            r.file, r.timestamp, single.2
                        );
                        unmatched += 1;
                        continue;
                    }
                }
            }
            Some(_) => {
                eprintln!("  Ambiguous filename match, skipping: {}", r.file);
                ambiguous += 1;
                continue;
            }
            None => {
                unmatched += 1;
                continue;
            }
        };
        if status == GradingStatus::Rejected as i32 {
            already_rejected += 1;
            continue;
        }
        let reason = r.regrade_reason.clone().unwrap_or_else(|| {
            format!(
                "[Auto] {} - score {:.2}{}",
                match &r.category {
                    Some(IssueCategory::PossibleObstruction) => "Obstruction",
                    Some(IssueCategory::LikelyClouds) => "Clouds",
                    _ => "Screening",
                },
                r.quality_score.unwrap_or(0.0),
                r.details
                    .as_deref()
                    .map(|d| format!("; {}", d))
                    .unwrap_or_default(),
            )
        });
        updates.push((id, GradingStatus::Rejected, Some(reason)));
    }

    println!(
        "\nRegrade against {}: {} rejects -> {} to update, {} already rejected, {} unmatched, {} ambiguous",
        db_path.display(),
        results.iter().filter(|r| r.verdict == Verdict::Reject).count(),
        updates.len(),
        already_rejected,
        unmatched,
        ambiguous,
    );

    if options.dry_run {
        for (id, _, reason) in &updates {
            println!(
                "  Would reject image {}: {}",
                id,
                reason.as_deref().unwrap_or("")
            );
        }
        println!("Dry run - no changes written.");
        return Ok(());
    }

    db.batch_update_grading_status(&updates)?;
    println!("Applied {} rejections.", updates.len());
    Ok(())
}

fn collect_fits_files(dir: &Path) -> Result<Vec<PathBuf>> {
    if dir.is_file() {
        return Ok(vec![dir.to_path_buf()]);
    }
    if !dir.is_dir() {
        return Err(anyhow::anyhow!(
            "Path does not exist or is not accessible: {}",
            dir.display()
        ));
    }
    let mut files = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d)? {
            let entry = entry?;
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("fits") || e.eq_ignore_ascii_case("fit"))
            {
                files.push(p);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// Analyze all frames, in parallel worker threads.
fn analyze_frames(files: &[PathBuf], options: &ScreenOptions) -> Result<Vec<FrameRecord>> {
    // Default to all cores (bounded by available memory); `--threads`
    // overrides. The old min(4) cap left most of a modern machine idle.
    // Foreground CLI work, so the interactive tier applies.
    let frame_pixels = files
        .first()
        .and_then(|p| crate::concurrency::probe_frame_pixels(p));
    let budget = crate::concurrency::plan_workers(
        options.threads,
        &crate::concurrency::WorkerPolicy::all_cores(),
        crate::concurrency::Priority::Interactive,
        frame_pixels,
    );
    eprintln!(
        "Analyzing with {} worker thread(s) — {}",
        budget.workers, budget.rationale
    );

    let done = AtomicUsize::new(0);
    let records: Mutex<Vec<FrameRecord>> = Mutex::new(Vec::with_capacity(files.len()));
    let total = files.len();

    crate::concurrency::parallel_index(total, budget.workers, |i| {
        let path = &files[i];
        match analyze_one_frame(path, options) {
            Ok(record) => {
                let n = done.fetch_add(1, Ordering::Relaxed) + 1;
                eprintln!(
                    "[{}/{}] {}: {} stars, hfr {:.2}, dead {}, bg spread {:.3}",
                    n,
                    total,
                    path.file_name().and_then(|s| s.to_str()).unwrap_or("?"),
                    record.star_count,
                    record.avg_hfr,
                    record
                        .dead_cell_fraction
                        .map(|d| format!("{:.0}%", d * 100.0))
                        .unwrap_or_else(|| "n/a".to_string()),
                    record.bg_cell_spread,
                );
                records.lock().unwrap().push(record);
            }
            Err(e) => {
                done.fetch_add(1, Ordering::Relaxed);
                eprintln!("Error analyzing {}: {}", path.display(), e);
            }
        }
    });

    Ok(records.into_inner().unwrap())
}

fn analyze_one_frame(path: &Path, options: &ScreenOptions) -> Result<FrameRecord> {
    let headers = extract_headers(path);
    let fits = FitsImage::from_file(path)?;
    let stats = fits.calculate_basic_statistics();

    let (star_count, avg_hfr, positions, catalog) = detect_stars(&fits, &stats, &options.detector)?;

    let calibration = PixelCalibration {
        adu_offset: fits.raw_min + fits.bzero,
        adu_per_stored: 1.0 / fits.raw_scale,
    };
    let spatial = compute_spatial_metrics(
        &positions,
        &fits.data,
        fits.width,
        fits.height,
        &calibration,
        &SpatialAnalysisConfig::default(),
    );

    Ok(FrameRecord {
        path: path.to_path_buf(),
        filter: headers.filter.unwrap_or_else(|| "unknown".to_string()),
        exposure_s: headers.exposure_s,
        timestamp: headers.timestamp,
        star_count,
        avg_hfr,
        median_adu: fits.stored_to_adu(stats.median),
        dead_cell_fraction: spatial.star_dead_cell_fraction,
        star_uniformity: spatial.star_uniformity,
        bg_cell_spread: spatial.bg_cell_spread,
        bg_cell_max_dev: spatial.bg_cell_max_dev,
        width: fits.width,
        height: fits.height,
        catalog,
        star_cell_counts: spatial.star_cell_counts,
        bg_cell_medians: spatial.bg_cell_medians,
        bg_glow_max: spatial.bg_glow_max,
        bg_glow_cells: spatial.bg_glow_cells,
        astrometry: None,
    })
}

/// (star_count, average_hfr, star centroid positions, photometric catalog)
type DetectionSummary = (usize, f64, Vec<(f64, f64)>, FrameCatalog);

fn detect_stars(
    fits: &FitsImage,
    stats: &crate::image_analysis::ImageStatistics,
    detector: &str,
) -> Result<DetectionSummary> {
    match detector.to_lowercase().as_str() {
        "nina" => {
            let params = StarDetectionParams {
                sensitivity: StarSensitivity::Normal,
                noise_reduction: NoiseReduction::None,
                use_roi: false,
            };
            let stretch_params = StretchParameters::default();
            let stretched = stretch_image(
                &fits.data,
                stats,
                stretch_params.factor,
                stretch_params.black_clipping,
            );
            let result = detect_stars_with_original(
                &stretched,
                &fits.data,
                fits.width,
                fits.height,
                &params,
            );
            let positions = result.star_list.iter().map(|s| s.position).collect();
            // The NINA port does not measure flux, so no photometric catalog.
            Ok((
                result.star_list.len(),
                result.average_hfr,
                positions,
                FrameCatalog::default(),
            ))
        }
        "hocusfocus" => {
            let params = HocusFocusParams::default();
            let result = detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);
            let positions = result.stars.iter().map(|s| s.position).collect();
            // Fluxes are background-subtracted sums in stored (per-frame
            // rescaled) units; divide by raw_scale for cross-frame ADU.
            let catalog = FrameCatalog {
                stars: result
                    .stars
                    .iter()
                    .filter(|s| s.flux > 0.0)
                    .map(|s| CatalogStar {
                        x: s.position.0,
                        y: s.position.1,
                        flux: s.flux / fits.raw_scale,
                    })
                    .collect(),
            };
            Ok((result.stars.len(), result.average_hfr, positions, catalog))
        }
        other => Err(anyhow::anyhow!("Unknown detector: {}", other)),
    }
}

#[derive(Debug, Default)]
pub(crate) struct FrameHeaders {
    pub(crate) filter: Option<String>,
    pub(crate) exposure_s: Option<f64>,
    pub(crate) timestamp: Option<i64>,
}

/// Extract filter, exposure and observation time from the FITS header.
pub(crate) fn extract_headers(path: &Path) -> FrameHeaders {
    let mut out = FrameHeaders::default();
    let Ok(headers) = seiza_fits::read_header(path) else {
        return out;
    };

    let find = |keys: &[&str]| -> Option<&seiza_fits::HeaderValue> {
        keys.iter()
            .find_map(|key| headers.iter().find(|(k, _)| k == key).map(|(_, v)| v))
    };
    out.filter = find(&["FILTER", "FILTERNAME"])
        .and_then(|v| v.as_str())
        .map(|v| v.trim().to_string());
    out.exposure_s = find(&["EXPTIME", "EXPOSURE"]).and_then(|v| v.as_f64());
    out.timestamp = find(&["DATE-OBS", "DATE-LOC"])
        .and_then(|v| v.as_str())
        .and_then(parse_fits_datetime);
    out
}

/// Parse a FITS DATE-OBS style timestamp ("2026-07-01T05:40:25.6971960")
/// into epoch seconds. Fractional seconds beyond nanoseconds are truncated.
fn parse_fits_datetime(s: &str) -> Option<i64> {
    let s = s.trim();
    // chrono handles at most 9 fractional digits; N.I.N.A. writes 7.
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(dt.and_utc().timestamp());
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp());
    }
    None
}

/// Group frames into (filter, exposure) sequences, run the sequence analyzer,
/// and attach verdicts.
fn score_records(
    records: &[FrameRecord],
    options: &ScreenOptions,
) -> (
    Vec<ScreenResult>,
    HashMap<usize, crate::photometry::FrameSignals>,
) {
    let config = SequenceAnalyzerConfig {
        session_gap_minutes: options.session_gap_minutes,
        dead_cell_rise_threshold: options.dead_cell_rise,
        ..Default::default()
    };
    let analyzer = SequenceAnalyzer::new(config.clone());

    // Group by (filter, exposure to the whole second): star counts are not
    // comparable across filters or exposure lengths.
    let mut groups: BTreeMap<(String, i64), Vec<usize>> = BTreeMap::new();
    for (idx, r) in records.iter().enumerate() {
        let exp_key = r.exposure_s.map(|e| e.round() as i64).unwrap_or(-1);
        groups
            .entry((r.filter.clone(), exp_key))
            .or_default()
            .push(idx);
    }

    let mut results: Vec<Option<ScreenResult>> = records
        .iter()
        .enumerate()
        .map(|(record_idx, r)| {
            Some(ScreenResult {
                record_idx,
                file: r
                    .path
                    .file_name()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
                    .to_string(),
                filter: r.filter.clone(),
                exposure_s: r.exposure_s,
                timestamp: r.timestamp,
                star_count: r.star_count,
                avg_hfr: r.avg_hfr,
                median_adu: r.median_adu,
                dead_cell_fraction: r.dead_cell_fraction,
                star_uniformity: r.star_uniformity,
                bg_cell_spread: r.bg_cell_spread,
                bg_cell_max_dev: r.bg_cell_max_dev,
                transparency: None,
                extinction_cell_fraction: None,
                star_cell_drop_fraction: None,
                bg_cell_rise_fraction: None,
                bg_cell_fall_fraction: None,
                bg_glow_max: (r.bg_glow_max > 0.0).then_some(r.bg_glow_max),
                quality_score: None,
                category: None,
                flags: Vec::new(),
                pointing: None,
                regrade_reason: None,
                details: None,
                verdict: Verdict::Ok,
            })
        })
        .collect();

    let spatial_config = SpatialAnalysisConfig::default();
    let grid = (spatial_config.grid_cols, spatial_config.grid_rows);
    let phot_config = PhotometryConfig::default();
    let gap_seconds = (options.session_gap_minutes * 60) as i64;
    let mut signals_by_idx: HashMap<usize, crate::photometry::FrameSignals> = HashMap::new();

    for ((filter, _exp), indices) in &groups {
        // Photometric + per-cell temporal signals, per session, before
        // scoring (records are pre-sorted by time, so indices are ordered).
        let timestamps: Vec<Option<i64>> = indices.iter().map(|&i| records[i].timestamp).collect();
        for session in split_sessions(&timestamps, gap_seconds) {
            let session_records: Vec<&FrameRecord> =
                session.iter().map(|&si| &records[indices[si]]).collect();
            let inputs: Vec<FrameInputs> = session_records
                .iter()
                .map(|r| FrameInputs {
                    catalog: r.catalog.clone(),
                    star_cell_counts: r.star_cell_counts.clone(),
                    bg_cell_medians: r.bg_cell_medians.clone(),
                })
                .collect();
            let (width, height) = session_records
                .first()
                .map(|r| (r.width, r.height))
                .unwrap_or((0, 0));
            let signals = sequence_screening_signals(&inputs, width, height, grid, &phot_config);
            for (si, sig) in session.iter().zip(signals) {
                signals_by_idx.insert(indices[*si], sig);
            }
        }

        let metrics: Vec<ImageMetrics> = indices
            .iter()
            .map(|&idx| {
                let r = &records[idx];
                let sig = signals_by_idx.get(&idx);
                ImageMetrics {
                    image_id: idx as i32,
                    timestamp: r.timestamp,
                    session_id: None,
                    star_count: Some(r.star_count as f64),
                    hfr: (r.avg_hfr > 0.0).then_some(r.avg_hfr),
                    eccentricity: None,
                    snr: None,
                    background: Some(r.median_adu),
                    dead_cell_fraction: r.dead_cell_fraction,
                    bg_cell_spread: Some(r.bg_cell_spread),
                    transparency: sig.and_then(|s| s.transparency),
                    extinction_cell_fraction: sig.and_then(|s| s.extinction_cell_fraction),
                    star_cell_drop_fraction: sig.and_then(|s| s.star_cell_drop_fraction),
                    bg_cell_rise_fraction: sig.and_then(|s| s.bg_cell_rise_fraction),
                    bg_cell_fall_fraction: sig.and_then(|s| s.bg_cell_fall_fraction),
                    bg_glow_max: (r.bg_glow_max > 0.0).then_some(r.bg_glow_max),
                    astrometry: r.astrometry.clone(),
                }
            })
            .collect();

        for (&idx, m) in indices.iter().zip(&metrics) {
            if let Some(res) = results[idx].as_mut() {
                res.transparency = m.transparency;
                res.extinction_cell_fraction = m.extinction_cell_fraction;
                res.star_cell_drop_fraction = m.star_cell_drop_fraction;
                res.bg_cell_rise_fraction = m.bg_cell_rise_fraction;
                res.bg_cell_fall_fraction = m.bg_cell_fall_fraction;
            }
        }

        for seq in analyzer.analyze(&metrics, 0, "screen", filter) {
            for img in seq.images {
                let idx = img.image_id as usize;
                if let Some(res) = results[idx].as_mut() {
                    res.quality_score = Some(img.quality_score);
                    res.category = img.category.clone();
                    res.flags = img.flags.clone();
                    res.pointing = img.pointing.clone();
                    res.regrade_reason = img.regrade_reason.clone();
                    res.details = img.details.clone();
                    res.verdict = verdict_for(
                        &img.quality_score,
                        &img.category,
                        img.regrade_reason.as_deref(),
                        options,
                    );
                }
            }
        }
    }

    (results.into_iter().flatten().collect(), signals_by_idx)
}

fn verdict_for(
    score: &f64,
    category: &Option<IssueCategory>,
    regrade_reason: Option<&str>,
    options: &ScreenOptions,
) -> Verdict {
    let rejectable = matches!(
        category,
        Some(IssueCategory::PossibleObstruction) | Some(IssueCategory::LikelyClouds)
    );
    if *score < options.min_score || rejectable || regrade_reason.is_some() {
        Verdict::Reject
    } else if category.is_some() || *score < options.min_score + 0.15 {
        Verdict::Warn
    } else {
        Verdict::Ok
    }
}

fn category_label(category: &Option<IssueCategory>) -> &'static str {
    match category {
        Some(IssueCategory::LikelyClouds) => "clouds",
        Some(IssueCategory::PossibleObstruction) => "obstruction",
        Some(IssueCategory::FocusDrift) => "focus-drift",
        Some(IssueCategory::TrackingError) => "tracking",
        Some(IssueCategory::WindShake) => "wind",
        Some(IssueCategory::SkyBrightening) => "sky-gradient",
        Some(IssueCategory::OffTarget) => "off-target",
        Some(IssueCategory::StableOffset) => "stable-offset",
        Some(IssueCategory::PointingJump) => "pointing-jump",
        Some(IssueCategory::PointingDrift) => "pointing-drift",
        Some(IssueCategory::PlateSolveFailed) => "unsolved",
        Some(IssueCategory::UnknownDegradation) => "unknown",
        None => "-",
    }
}

fn print_table(results: &[ScreenResult]) {
    println!(
        "{:<52} {:>6} {:>6} {:>6} {:>7} {:>6} {:>8} {:>6} {:>5} {:>6} {:>13} {:>7}",
        "File",
        "Filter",
        "Stars",
        "HFR",
        "MedADU",
        "Dead%",
        "BgSpread",
        "Transp",
        "Ext%",
        "Score",
        "Category",
        "Verdict"
    );
    println!("{}", "-".repeat(139));
    for r in results {
        println!(
            "{:<52} {:>6} {:>6} {:>6.2} {:>7.0} {:>6} {:>8.3} {:>6} {:>5} {:>6} {:>13} {:>7}",
            truncate_name(&r.file, 52),
            r.filter,
            r.star_count,
            r.avg_hfr,
            r.median_adu,
            r.dead_cell_fraction
                .map(|d| format!("{:.0}", d * 100.0))
                .unwrap_or_else(|| "-".to_string()),
            r.bg_cell_spread,
            r.transparency
                .map(|t| format!("{:.2}", t))
                .unwrap_or_else(|| "-".to_string()),
            r.extinction_cell_fraction
                .map(|e| format!("{:.0}", e * 100.0))
                .unwrap_or_else(|| "-".to_string()),
            r.quality_score
                .map(|s| format!("{:.2}", s))
                .unwrap_or_else(|| "-".to_string()),
            category_label(&r.category),
            match r.verdict {
                Verdict::Ok => "OK",
                Verdict::Warn => "WARN",
                Verdict::Reject => "REJECT",
            },
        );
    }

    let total = results.len();
    let rejects = results
        .iter()
        .filter(|r| r.verdict == Verdict::Reject)
        .count();
    let warns = results
        .iter()
        .filter(|r| r.verdict == Verdict::Warn)
        .count();
    println!("{}", "-".repeat(139));
    println!(
        "{} frames: {} ok, {} warn, {} reject",
        total,
        total - rejects - warns,
        warns,
        rejects
    );
}

fn truncate_name(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    // Byte-based cut point nudged forward to the next char boundary so
    // multi-byte characters can never cause a slice panic.
    let mut cut = name.len().saturating_sub(max.saturating_sub(3));
    while cut < name.len() && !name.is_char_boundary(cut) {
        cut += 1;
    }
    format!("...{}", &name[cut..])
}

fn print_csv(results: &[ScreenResult]) {
    println!(
        "File,Filter,ExposureS,Timestamp,Stars,AvgHFR,MedianADU,DeadCellFraction,StarUniformity,BgCellSpread,BgCellMaxDev,Transparency,ExtinctionCellFraction,StarCellDropFraction,BgCellRiseFraction,BgCellFallFraction,BgGlowMax,Score,Category,Verdict,SolveState,OffsetFieldFraction,RegradeReason"
    );
    for r in results {
        println!(
            "{},{},{},{},{},{:.3},{:.1},{},{},{:.4},{:.4},{},{},{},{},{},{},{},{},{},{},{},{}",
            r.file,
            r.filter,
            r.exposure_s.map(|e| e.to_string()).unwrap_or_default(),
            r.timestamp.map(|t| t.to_string()).unwrap_or_default(),
            r.star_count,
            r.avg_hfr,
            r.median_adu,
            r.dead_cell_fraction
                .map(|d| format!("{:.4}", d))
                .unwrap_or_default(),
            r.star_uniformity
                .map(|u| format!("{:.4}", u))
                .unwrap_or_default(),
            r.bg_cell_spread,
            r.bg_cell_max_dev,
            r.transparency
                .map(|v| format!("{:.4}", v))
                .unwrap_or_default(),
            r.extinction_cell_fraction
                .map(|v| format!("{:.4}", v))
                .unwrap_or_default(),
            r.star_cell_drop_fraction
                .map(|v| format!("{:.4}", v))
                .unwrap_or_default(),
            r.bg_cell_rise_fraction
                .map(|v| format!("{:.4}", v))
                .unwrap_or_default(),
            r.bg_cell_fall_fraction
                .map(|v| format!("{:.4}", v))
                .unwrap_or_default(),
            r.bg_glow_max
                .map(|v| format!("{:.4}", v))
                .unwrap_or_default(),
            r.quality_score
                .map(|s| format!("{:.4}", s))
                .unwrap_or_default(),
            category_label(&r.category),
            r.verdict,
            r.pointing
                .as_ref()
                .map(|pointing| if pointing.pixel_solved {
                    "solved"
                } else if pointing.solve_failed && pointing.image_quality_evidence {
                    "unsolved"
                } else {
                    "unavailable"
                })
                .unwrap_or_default(),
            r.pointing
                .as_ref()
                .and_then(|pointing| pointing.field_fraction_offset)
                .map(|fraction| format!("{:.3}", fraction))
                .unwrap_or_default(),
            r.regrade_reason
                .as_deref()
                .map(|reason| format!("\"{}\"", reason.replace('"', "\"\"")))
                .unwrap_or_default(),
        );
    }
}

fn print_json(results: &[ScreenResult]) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(results)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_nina_date_obs() {
        // N.I.N.A. writes 7 fractional digits.
        let ts = parse_fits_datetime("2026-07-01T05:40:25.6971960").unwrap();
        assert_eq!(ts, 1782884425);
        assert!(parse_fits_datetime("2024-01-15T22:00:00").is_some());
        assert!(parse_fits_datetime("garbage").is_none());
    }

    #[test]
    fn truncate_name_is_utf8_safe() {
        // Regression (code review): multi-byte characters at the cut point
        // must not panic.
        let name = "Große_Magellansche_Wolke_Hα_2026-06-30_22-40-25_R_-10.10_60.00s_0008.fits";
        let out = truncate_name(name, 52);
        assert!(out.starts_with("..."));
        assert!(out.len() <= 56);
        // All-multibyte name with the cut landing mid-character.
        let cjk = "银河系目标名称非常长需要截断的文件名称测试用例数据.fits";
        let out = truncate_name(cjk, 20);
        assert!(out.starts_with("..."));
    }

    #[test]
    fn verdict_thresholds() {
        let options = ScreenOptions {
            detector: "hocusfocus".into(),
            format: "table".into(),
            min_score: 0.35,
            dead_cell_rise: 0.08,
            threads: None,
            session_gap_minutes: 60,
            regrade_db: None,
            dry_run: false,
            registry: None,
            annotate_dir: None,
        };
        assert_eq!(verdict_for(&0.9, &None, None, &options), Verdict::Ok);
        assert_eq!(verdict_for(&0.2, &None, None, &options), Verdict::Reject);
        assert_eq!(
            verdict_for(
                &0.9,
                &Some(IssueCategory::PossibleObstruction),
                None,
                &options
            ),
            Verdict::Reject,
            "occlusion rejects regardless of composite score"
        );
        assert_eq!(
            verdict_for(&0.8, &Some(IssueCategory::SkyBrightening), None, &options),
            Verdict::Warn,
            "gradients are recoverable: warn, not reject"
        );
        assert_eq!(verdict_for(&0.45, &None, None, &options), Verdict::Warn);
        assert_eq!(
            verdict_for(
                &0.9,
                &Some(IssueCategory::OffTarget),
                Some("off target"),
                &options
            ),
            Verdict::Reject
        );
    }
}
