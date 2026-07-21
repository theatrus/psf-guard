//! Import folders of FITS light frames into a Target Scheduler database.
//!
//! Works with `crate::ts_schema` (create a brand-new DB) or against any
//! existing v22+ scheduler database. Frames are grouped into projects /
//! targets / exposure plans by `grouping::build_plan`; every inserted row
//! carries a fresh GUID exactly as the plugin writes them (TS 5, schema v22+).
//!
//! Import is header-only and fast: `gradingStatus` starts Pending and star /
//! quality metrics are intentionally absent from the metadata JSON (readers
//! treat missing keys as None). The quality backfill is a separate pass —
//! the server kicks its existing quality scan after an import job finishes.
//!
//! Idempotency: a frame whose basename already appears in any
//! `acquiredimage.metadata` FileName is skipped, so re-running an import on a
//! folder that gained new subs only adds the new ones.

pub mod grouping;
pub mod headers;

use crate::ts_schema::new_guid;
use anyhow::{bail, Context, Result};
use chrono::TimeZone;
use grouping::{build_plan, ImportPlan, TemplateKey, DEFAULT_TIME_GAP_DAYS};
use headers::FrameMeta;
use rayon::prelude::*;
use rusqlite::{params, Connection};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct ImportOptions {
    /// Gap between consecutive frames (same rig) that starts a new project.
    pub time_gap_days: f64,
    /// Profile to attach imported rows to. Defaults to the database's single
    /// existing profile, or a freshly created one.
    pub profile_id: Option<String>,
    /// Build the plan and run every insert, then roll the transaction back.
    pub dry_run: bool,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            time_gap_days: DEFAULT_TIME_GAP_DAYS,
            profile_id: None,
            dry_run: false,
        }
    }
}

/// Per-project report line.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProjectSummary {
    pub name: String,
    pub targets: usize,
    pub frames: usize,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImportOutcome {
    pub scanned: usize,
    pub unreadable: usize,
    pub non_light: usize,
    pub skipped_existing: usize,
    pub imported: usize,
    pub projects_created: usize,
    pub targets_created: usize,
    pub templates_created: usize,
    pub templates_reused: usize,
    pub plans_created: usize,
    pub profile_id: String,
    pub dry_run: bool,
    pub project_summaries: Vec<ProjectSummary>,
    /// Target row ids created by this import (live runs only) — the server's
    /// post-import quality backfill iterates these.
    pub created_target_ids: Vec<i32>,
}

/// Recursively collect `.fits` / `.fit` files (a bare file argument is
/// accepted too). Sorted for deterministic plans.
pub fn collect_fits_files(dirs: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for dir in dirs {
        if dir.is_file() {
            files.push(dir.clone());
            continue;
        }
        if !dir.is_dir() {
            bail!(
                "Path does not exist or is not accessible: {}",
                dir.display()
            );
        }
        let mut stack = vec![dir.clone()];
        while let Some(d) = stack.pop() {
            for entry in std::fs::read_dir(&d)
                .with_context(|| format!("reading directory {}", d.display()))?
            {
                let p = entry?.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().and_then(|e| e.to_str()).is_some_and(|e| {
                    e.eq_ignore_ascii_case("fits") || e.eq_ignore_ascii_case("fit")
                }) {
                    files.push(p);
                }
            }
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Read headers for every file, in parallel (header-only, I/O bound).
pub fn scan_frames(files: &[PathBuf]) -> Vec<FrameMeta> {
    scan_frames_counted(files, &std::sync::atomic::AtomicUsize::new(0))
}

/// [`scan_frames`], bumping `progress` after each file so a caller on another
/// thread (the server import job) can report scan progress.
pub fn scan_frames_counted(
    files: &[PathBuf],
    progress: &std::sync::atomic::AtomicUsize,
) -> Vec<FrameMeta> {
    files
        .par_iter()
        .map(|path| {
            let meta = headers::read_frame_meta(path);
            progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            meta
        })
        .collect()
}

/// Import scanned frames into an open scheduler database. The whole import is
/// one transaction; on `dry_run` it is rolled back after counting.
pub fn import_frames(
    conn: &mut Connection,
    frames: Vec<FrameMeta>,
    options: &ImportOptions,
) -> Result<ImportOutcome> {
    let mut outcome = ImportOutcome {
        scanned: frames.len(),
        dry_run: options.dry_run,
        ..Default::default()
    };

    let tx = conn.transaction().context("starting import transaction")?;

    let existing = existing_basenames(&tx)?;
    let mut lights: Vec<FrameMeta> = Vec::new();
    for frame in frames {
        if !frame.readable {
            outcome.unreadable += 1;
        } else if !frame.is_light() {
            outcome.non_light += 1;
        } else if existing.contains(&frame.basename().to_lowercase()) {
            outcome.skipped_existing += 1;
        } else {
            lights.push(frame);
        }
    }

    let plan = build_plan(&lights, options.time_gap_days);
    let profile_id = resolve_profile(&tx, options)?;
    let template_ids = ensure_templates(&tx, &plan, &profile_id, &mut outcome)?;

    for project in &plan.projects {
        let project_id = insert_project(&tx, project, &profile_id)?;
        outcome.projects_created += 1;
        outcome.project_summaries.push(ProjectSummary {
            name: project.name.clone(),
            targets: project.targets.len(),
            frames: project.frame_count(),
        });

        for target in &project.targets {
            let target_id = insert_target(&tx, target, project_id)?;
            outcome.targets_created += 1;
            outcome.created_target_ids.push(target_id as i32);

            for exposure in &target.exposures {
                let template_id = *template_ids
                    .get(&exposure.template)
                    .expect("template inserted for every plan key");
                let plan_id = insert_exposure_plan(
                    &tx,
                    &profile_id,
                    exposure.exposure_s,
                    exposure.frames.len(),
                    target_id,
                    template_id,
                )?;
                outcome.plans_created += 1;

                for &frame_idx in &exposure.frames {
                    insert_acquired_image(
                        &tx,
                        &lights[frame_idx],
                        project_id,
                        target_id,
                        plan_id,
                        &profile_id,
                    )?;
                    outcome.imported += 1;
                }
            }
        }
    }

    outcome.profile_id = profile_id;
    if options.dry_run {
        tx.rollback().context("rolling back dry-run transaction")?;
        // Rolled-back row ids mean nothing to callers (backfill would scan
        // targets that don't exist).
        outcome.created_target_ids.clear();
    } else {
        tx.commit().context("committing import transaction")?;
    }
    Ok(outcome)
}

/// Basenames (lowercased) of every FileName already present in the DB.
fn existing_basenames(conn: &Connection) -> Result<HashSet<String>> {
    let mut set = HashSet::new();
    let mut stmt = conn.prepare("SELECT metadata FROM acquiredimage")?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    for metadata in rows.flatten() {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&metadata)
            && let Some(file_name) = value.get("FileName").and_then(|v| v.as_str())
        {
            // TS stores Windows paths; take the basename across either
            // separator convention.
            let base = file_name
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(file_name)
                .to_lowercase();
            if !base.is_empty() {
                set.insert(base);
            }
        }
    }
    Ok(set)
}

/// Pick (or create) the profile the imported rows belong to.
fn resolve_profile(conn: &Connection, options: &ImportOptions) -> Result<String> {
    if let Some(profile_id) = &options.profile_id {
        ensure_profile_preference(conn, profile_id)?;
        return Ok(profile_id.clone());
    }

    let mut stmt = conn.prepare("SELECT DISTINCT profileId FROM profilepreference")?;
    let profiles: Vec<String> = stmt
        .query_map([], |row| row.get(0))?
        .collect::<Result<_, _>>()?;
    match profiles.len() {
        0 => {
            let profile_id = new_guid();
            ensure_profile_preference(conn, &profile_id)?;
            Ok(profile_id)
        }
        1 => Ok(profiles.into_iter().next().unwrap()),
        n => bail!(
            "database has {} profiles; pass --profile-id to pick one of: {}",
            n,
            profiles.join(", ")
        ),
    }
}

/// Insert a `profilepreference` row with the plugin's constructor defaults
/// (TS 5 `ProfilePreference(string profileId)`), if none exists yet.
fn ensure_profile_preference(conn: &Connection, profile_id: &str) -> Result<()> {
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM profilepreference WHERE profileId = ?1",
        params![profile_id],
        |row| row.get(0),
    )?;
    if exists > 0 {
        return Ok(());
    }
    conn.execute(
        "INSERT INTO profilepreference (
            profileId,
            enableGradeRMS, enableGradeStars, enableGradeHFR,
            enableGradeFWHM, enableGradeEccentricity, enableMoveRejected,
            maxGradingSampleSize, rmsPixelThreshold,
            detectedStarsSigmaFactor, hfrSigmaFactor,
            fwhmSigmaFactor, eccentricitySigmaFactor,
            acceptimprovement, exposurethrottle, parkonwait,
            enableSmartPlanWindow,
            enableSynchronization, syncWaitTimeout, syncActionTimeout,
            syncSolveRotateTimeout, syncEventContainerTimeout,
            enableDeleteAcquiredImagesWithTarget,
            delayGrading, autoAcceptLevelHFR, autoAcceptLevelFWHM,
            autoAcceptLevelEccentricity,
            enableSimulatedRun, skipSimulatedWaits, skipSimulatedUpdates,
            enableSlewCenter, logLevel, enableStopOnHumidity,
            enableProfileTargetCompletionReset,
            enableAPI, apiPort, apiPrettyPrint,
            guid
        ) VALUES (
            ?1,
            1, 1, 1,
            0, 0, 0,
            10, 8,
            4, 4,
            4, 4,
            1, 125, 0,
            1,
            0, 300, 300,
            300, 300,
            1,
            80, 0, 0,
            0,
            0, 1, 0,
            1, 3, 1,
            0,
            0, 8188, 0,
            ?2
        )",
        params![profile_id, new_guid()],
    )?;
    Ok(())
}

/// Default rule-weight rows every TS project carries
/// (`ScoringRule.GetDefaultRuleWeights()`, weights are `factor * 100`).
const DEFAULT_RULE_WEIGHTS: &[(&str, f64)] = &[
    ("Meridian Flip Penalty", 0.0),
    ("Meridian Window Priority", 75.0),
    ("Mosaic Completion", 0.0),
    ("Percent Complete", 50.0),
    ("Project Priority", 50.0),
    ("Setting Soonest", 50.0),
    ("Smart Exposure Order", 0.0),
    ("Target Switch Penalty", 67.0),
];

/// Find-or-create the exposure template for every plan key. Templates are
/// per-profile in TS, so re-imports reuse rows from earlier runs.
fn ensure_templates(
    conn: &Connection,
    plan: &ImportPlan,
    profile_id: &str,
    outcome: &mut ImportOutcome,
) -> Result<HashMap<TemplateKey, i64>> {
    // Most-frequent exposure per template (by frame count) seeds
    // `defaultexposure`; TS's constructor default of 60 covers the rest.
    let mut frames_per_exposure: HashMap<(TemplateKey, i64), usize> = HashMap::new();
    for project in &plan.projects {
        for target in &project.targets {
            for exposure in &target.exposures {
                *frames_per_exposure
                    .entry((
                        exposure.template.clone(),
                        (exposure.exposure_s * 1000.0).round() as i64,
                    ))
                    .or_default() += exposure.frames.len();
            }
        }
    }
    let mut ids = HashMap::new();
    for key in plan.template_keys() {
        let existing: Option<i64> = conn
            .query_row(
                "SELECT Id FROM exposuretemplate
                 WHERE profileId = ?1 AND filtername = ?2
                   AND IFNULL(gain, -1) = ?3 AND IFNULL(offset, -1) = ?4
                   AND IFNULL(bin, 1) = ?5 AND IFNULL(readoutmode, -1) = ?6",
                params![
                    profile_id,
                    key.filter,
                    key.gain,
                    key.offset,
                    key.binning,
                    key.readout
                ],
                |row| row.get(0),
            )
            .ok();
        if let Some(id) = existing {
            outcome.templates_reused += 1;
            ids.insert(key, id);
            continue;
        }

        let default_exposure = frames_per_exposure
            .iter()
            .filter(|((k, _), _)| *k == key)
            .max_by_key(|(_, count)| **count)
            .map(|((_, exp_ms), _)| *exp_ms as f64 / 1000.0)
            .filter(|e| *e > 0.0)
            .unwrap_or(60.0);

        conn.execute(
            "INSERT INTO exposuretemplate (
                profileId, name, filtername, gain, offset, bin, readoutmode,
                twilightlevel, moonavoidanceenabled, moonavoidanceseparation,
                moonavoidancewidth, maximumhumidity, defaultexposure,
                moonrelaxscale, moonrelaxmaxaltitude, moonrelaxminaltitude,
                moondownenabled, ditherevery, minutesOffset, guid
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, 0, 60, 7, 0, ?8, 0, 5, -15, 0, -1, 0, ?9)",
            params![
                profile_id,
                key.name(),
                key.filter,
                key.gain,
                key.offset,
                key.binning,
                key.readout,
                default_exposure,
                new_guid(),
            ],
        )?;
        outcome.templates_created += 1;
        ids.insert(key, conn.last_insert_rowid());
    }
    Ok(ids)
}

/// Insert a project row with the plugin's constructor defaults
/// (`Project(string profileId)`: Draft state, Normal priority) plus its
/// default rule weights.
fn insert_project(
    conn: &Connection,
    project: &grouping::PlannedProject,
    profile_id: &str,
) -> Result<i64> {
    let description = format!(
        "Imported by PSF Guard ({} frames{})",
        project.frame_count(),
        match (project.start_ts, project.end_ts) {
            (Some(start), Some(end)) => format!(", {} – {}", format_date(start), format_date(end)),
            _ => String::new(),
        }
    );
    conn.execute(
        "INSERT INTO project (
            profileId, name, description, state, priority, createdate,
            activedate, inactivedate, minimumtime, minimumaltitude,
            maximumAltitude, usecustomhorizon, horizonoffset, meridianwindow,
            filterswitchfrequency, ditherevery, enablegrader, isMosaic,
            flatsHandling, smartexposureorder, guid
        ) VALUES (?1, ?2, ?3, 0, 1, ?4, NULL, NULL, 30, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, ?5)",
        params![
            profile_id,
            project.name,
            description,
            chrono::Utc::now().timestamp(),
            new_guid(),
        ],
    )?;
    let project_id = conn.last_insert_rowid();

    for (name, weight) in DEFAULT_RULE_WEIGHTS {
        conn.execute(
            "INSERT INTO ruleweight (name, weight, projectid) VALUES (?1, ?2, ?3)",
            params![name, weight, project_id],
        )?;
    }
    Ok(project_id)
}

fn insert_target(
    conn: &Connection,
    target: &grouping::PlannedTarget,
    project_id: i64,
) -> Result<i64> {
    // Constructor defaults: active, epoch J2000 (N.I.N.A. enum value 2),
    // rotation 0, ROI 100%. RA is stored in decimal HOURS (TS convention).
    conn.execute(
        "INSERT INTO target (name, active, ra, dec, epochcode, rotation, roi, projectid, guid)
         VALUES (?1, 1, ?2, ?3, 2, 0, 100, ?4, ?5)",
        params![
            target.name,
            target.ra_hours.unwrap_or(0.0),
            target.dec_deg.unwrap_or(0.0),
            project_id,
            new_guid(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_exposure_plan(
    conn: &Connection,
    profile_id: &str,
    exposure_s: f64,
    frame_count: usize,
    target_id: i64,
    template_id: i64,
) -> Result<i64> {
    // Frames start Pending, so acquired = count and accepted = 0; grading /
    // backfill later flips accepted (TS's own convention after regrade).
    conn.execute(
        "INSERT INTO exposureplan (
            profileId, exposure, desired, acquired, accepted,
            targetid, exposureTemplateId, enabled, guid
        ) VALUES (?1, ?2, ?3, ?4, 0, ?5, ?6, 1, ?7)",
        params![
            profile_id,
            if exposure_s > 0.0 { exposure_s } else { -1.0 },
            frame_count as i64,
            frame_count as i64,
            target_id,
            template_id,
            new_guid(),
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

fn insert_acquired_image(
    conn: &Connection,
    frame: &FrameMeta,
    project_id: i64,
    target_id: i64,
    exposure_plan_id: i64,
    profile_id: &str,
) -> Result<()> {
    let metadata = frame_metadata_json(frame);
    conn.execute(
        "INSERT INTO acquiredimage (
            projectId, targetId, acquireddate, filtername, gradingStatus,
            metadata, rejectreason, profileId, exposureId, guid
        ) VALUES (?1, ?2, ?3, ?4, 0, ?5, NULL, ?6, ?7, ?8)",
        params![
            project_id,
            target_id,
            frame.timestamp,
            frame.filter.as_deref().unwrap_or("NONE"),
            metadata,
            profile_id,
            exposure_plan_id,
            new_guid(),
        ],
    )?;
    Ok(())
}

/// Build the `acquiredimage.metadata` JSON. Shape follows the plugin's
/// `ImageMetadata` DTO; keys whose values we cannot know from headers
/// (star metrics, ADU stats, guiding RMS) are omitted entirely — readers
/// treat missing keys as None, while zeros would read as measurements.
fn frame_metadata_json(frame: &FrameMeta) -> String {
    let mut map = serde_json::Map::new();
    let mut put = |key: &str, value: serde_json::Value| {
        map.insert(key.to_string(), value);
    };

    put(
        "FileName",
        serde_json::Value::String(frame.path.to_string_lossy().into_owned()),
    );
    put("SessionId", 0.into());
    if let Some(filter) = &frame.filter {
        put("FilterName", filter.clone().into());
    }
    if let Some(ts) = frame.timestamp
        && let Some(dt) = chrono::Utc.timestamp_opt(ts, 0).single()
    {
        put(
            "ExposureStartTime",
            dt.to_rfc3339_opts(chrono::SecondsFormat::Secs, true).into(),
        );
    }
    if let Some(exposure) = frame.exposure_s {
        put("ExposureDuration", exposure.into());
    }
    if let Some(gain) = frame.gain {
        put("Gain", gain.into());
    }
    if let Some(offset) = frame.offset {
        put("Offset", offset.into());
    }
    if let Some(bx) = frame.binning_x {
        put(
            "Binning",
            format!("{}x{}", bx, frame.binning_y.unwrap_or(bx)).into(),
        );
    }
    if let Some(mode) = frame.readout_mode {
        put("ReadoutMode", mode.into());
    }
    put("ROI", 100.0.into());
    if let Some(position) = frame.focuser_position {
        put("FocuserPosition", position.into());
    }
    if let Some(temp) = frame.focuser_temp {
        put("FocuserTemp", temp.into());
    }
    if let Some(position) = frame.rotator_position {
        put("RotatorPosition", position.into());
    }
    if let Some(side) = &frame.pier_side {
        put("PierSide", side.clone().into());
    }
    if let Some(temp) = frame.camera_temp {
        put("CameraTemp", temp.into());
    }
    if let Some(temp) = frame.camera_target_temp {
        put("CameraTargetTemp", temp.into());
    }
    if let Some(airmass) = frame.airmass {
        put("Airmass", airmass.into());
    }

    serde_json::Value::Object(map).to_string()
}

fn format_date(ts: i64) -> String {
    chrono::Utc
        .timestamp_opt(ts, 0)
        .single()
        .map(|dt| dt.format("%Y-%m-%d").to_string())
        .unwrap_or_default()
}

/// CLI-facing report printer shared by `create-db` and `import`.
pub fn print_outcome(outcome: &ImportOutcome) {
    let mode = if outcome.dry_run {
        "(dry-run — rolled back)"
    } else {
        "(live)"
    };
    println!("\nImport {}:", mode);
    println!("  Scanned:          {}", outcome.scanned);
    if outcome.unreadable > 0 {
        println!("  Unreadable:       {}", outcome.unreadable);
    }
    if outcome.non_light > 0 {
        println!("  Non-light frames: {}", outcome.non_light);
    }
    if outcome.skipped_existing > 0 {
        println!("  Already in DB:    {}", outcome.skipped_existing);
    }
    println!("  Imported:         {}", outcome.imported);
    println!(
        "  Projects: {}  Targets: {}  Exposure plans: {}  Templates: {} new / {} reused",
        outcome.projects_created,
        outcome.targets_created,
        outcome.plans_created,
        outcome.templates_created,
        outcome.templates_reused,
    );
    println!("  Profile: {}", outcome.profile_id);
    for summary in &outcome.project_summaries {
        println!(
            "    {} — {} target(s), {} frame(s)",
            summary.name, summary.targets, summary.frames
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts_schema;

    fn light(object: &str, filter: &str, ts: i64) -> FrameMeta {
        FrameMeta {
            path: PathBuf::from(format!("/data/{object}_{filter}_{ts}.fits")),
            readable: true,
            image_type: Some("LIGHT".into()),
            object: Some(object.into()),
            filter: Some(filter.into()),
            timestamp: Some(ts),
            date_obs: Some("2026-01-01T00:00:00".into()),
            exposure_s: Some(300.0),
            gain: Some(100),
            offset: Some(30),
            binning_x: Some(1),
            binning_y: Some(1),
            ra_deg: Some(10.68),
            dec_deg: Some(41.27),
            telescope: Some("EdgeHD".into()),
            camera: Some("ASI2600".into()),
            focal_length_mm: Some(1960.0),
            ..Default::default()
        }
    }

    fn fresh_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        ts_schema::apply_schema(&conn).unwrap();
        conn
    }

    #[test]
    fn imports_into_fresh_db() {
        let mut conn = fresh_conn();
        let frames = vec![
            light("M31", "Ha", 1_000),
            light("M31", "OIII", 2_000),
            light("M33", "Ha", 3_000),
        ];
        let outcome = import_frames(&mut conn, frames, &ImportOptions::default()).unwrap();
        assert_eq!(outcome.imported, 3);
        assert_eq!(outcome.projects_created, 1);
        assert_eq!(outcome.targets_created, 2);
        assert_eq!(outcome.templates_created, 2);
        assert_eq!(outcome.plans_created, 3);

        // Rows landed and carry guids + Pending status.
        let (count, with_guid): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COUNT(guid) FROM acquiredimage WHERE gradingStatus = 0",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(count, 3);
        assert_eq!(with_guid, 3);

        // RA stored in hours.
        let ra: f64 = conn
            .query_row("SELECT ra FROM target WHERE name = 'M31'", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert!((ra - 10.68 / 15.0).abs() < 1e-9);
        let epoch: i64 = conn
            .query_row(
                "SELECT epochcode FROM target WHERE name = 'M31'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(epoch, 2);

        // Rule weights exist for the project.
        let weights: i64 = conn
            .query_row("SELECT COUNT(*) FROM ruleweight", [], |row| row.get(0))
            .unwrap();
        assert_eq!(weights, 8);

        // Metadata parses and has no fabricated star metrics.
        let metadata: String = conn
            .query_row("SELECT metadata FROM acquiredimage LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&metadata).unwrap();
        assert!(value.get("FileName").is_some());
        assert!(value.get("DetectedStars").is_none());
        assert!(value.get("HFR").is_none());
    }

    #[test]
    fn reimport_skips_existing_by_basename() {
        let mut conn = fresh_conn();
        let frames = vec![light("M31", "Ha", 1_000)];
        import_frames(&mut conn, frames.clone(), &ImportOptions::default()).unwrap();

        let outcome = import_frames(&mut conn, frames, &ImportOptions::default()).unwrap();
        assert_eq!(outcome.skipped_existing, 1);
        assert_eq!(outcome.imported, 0);
        assert_eq!(outcome.projects_created, 0);

        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |row| row.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn incremental_import_reuses_templates() {
        let mut conn = fresh_conn();
        import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions::default(),
        )
        .unwrap();

        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 2_000)],
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(outcome.imported, 1);
        assert_eq!(outcome.templates_created, 0);
        assert_eq!(outcome.templates_reused, 1);

        let templates: i64 = conn
            .query_row("SELECT COUNT(*) FROM exposuretemplate", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(templates, 1);
    }

    #[test]
    fn dry_run_rolls_back() {
        let mut conn = fresh_conn();
        let options = ImportOptions {
            dry_run: true,
            ..Default::default()
        };
        let outcome = import_frames(&mut conn, vec![light("M31", "Ha", 1_000)], &options).unwrap();
        assert_eq!(outcome.imported, 1);
        for table in ["acquiredimage", "project", "target", "profilepreference"] {
            let count: i64 = conn
                .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table} not rolled back");
        }
    }

    #[test]
    fn calibration_frames_are_skipped() {
        let mut conn = fresh_conn();
        let mut dark = light("M31", "Ha", 1_000);
        dark.image_type = Some("DARK".into());
        let outcome = import_frames(&mut conn, vec![dark], &ImportOptions::default()).unwrap();
        assert_eq!(outcome.non_light, 1);
        assert_eq!(outcome.imported, 0);
    }

    #[test]
    fn multiple_profiles_require_explicit_choice() {
        let mut conn = fresh_conn();
        ensure_profile_preference(&conn, "profile-a").unwrap();
        ensure_profile_preference(&conn, "profile-b").unwrap();
        let err = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--profile-id"), "unexpected error: {err}");
    }

    #[test]
    fn explicit_profile_is_used_and_created() {
        let mut conn = fresh_conn();
        let options = ImportOptions {
            profile_id: Some("my-profile".into()),
            ..Default::default()
        };
        let outcome = import_frames(&mut conn, vec![light("M31", "Ha", 1_000)], &options).unwrap();
        assert_eq!(outcome.profile_id, "my-profile");
        let profile: String = conn
            .query_row("SELECT profileId FROM acquiredimage LIMIT 1", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(profile, "my-profile");
        // The profilepreference row was created with the constructor guid.
        let guid: Option<String> = conn
            .query_row(
                "SELECT guid FROM profilepreference WHERE profileId = 'my-profile'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(guid.is_some());
    }

    #[test]
    fn exposure_plan_counts_track_frames() {
        let mut conn = fresh_conn();
        import_frames(
            &mut conn,
            vec![
                light("M31", "Ha", 1_000),
                light("M31", "Ha", 2_000),
                light("M31", "Ha", 3_000),
            ],
            &ImportOptions::default(),
        )
        .unwrap();
        let (desired, acquired, accepted, enabled): (i64, i64, i64, i64) = conn
            .query_row(
                "SELECT desired, acquired, accepted, enabled FROM exposureplan",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!((desired, acquired, accepted, enabled), (3, 3, 0, 1));
    }
}
