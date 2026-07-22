//! Import folders of FITS light frames into a Target Scheduler database.
//!
//! Works with `crate::ts_schema` (create a brand-new DB) or against any
//! existing v22+ scheduler database. Frames are grouped into projects /
//! targets / exposure plans by `grouping::build_plan`; every inserted row
//! carries a fresh GUID exactly as the plugin writes them (TS 5, schema v22+).
//!
//! Import is header-only and fast: `gradingStatus` starts Pending and star /
//! quality metrics are intentionally absent from the metadata JSON (readers
//! treat missing keys as None). Database-wide quality analysis is a separate,
//! on-demand background job; imports may queue it but never wait for it.
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
    /// Attach frames to EXISTING targets (matched by name, then by
    /// coordinates) instead of synthesizing new structure for them. New
    /// projects/targets are only created for frames nothing matches. This is
    /// what keeps an import into a real scheduler database from duplicating
    /// its projects — disable only for deliberate ground-zero imports.
    pub attach_existing: bool,
    /// Coordinate-match radius for attaching to an existing target, in
    /// degrees of angular separation.
    pub match_radius_deg: f64,
}

pub const DEFAULT_MATCH_RADIUS_DEG: f64 = 0.5;

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            time_gap_days: DEFAULT_TIME_GAP_DAYS,
            profile_id: None,
            dry_run: false,
            attach_existing: true,
            match_radius_deg: DEFAULT_MATCH_RADIUS_DEG,
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

/// One existing target that received attached frames.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AttachSummary {
    pub project: String,
    pub target: String,
    pub frames: usize,
    /// `name` or `coordinates` — how the match was made.
    pub matched_by: String,
}

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct ImportOutcome {
    pub scanned: usize,
    pub unreadable: usize,
    pub non_light: usize,
    pub skipped_existing: usize,
    pub imported: usize,
    /// Frames attached to targets that already existed in the database.
    pub attached: usize,
    pub projects_created: usize,
    pub targets_created: usize,
    pub templates_created: usize,
    pub templates_reused: usize,
    pub plans_created: usize,
    pub profile_id: String,
    pub dry_run: bool,
    pub project_summaries: Vec<ProjectSummary>,
    /// Existing targets that gained frames, for the preview/confirm UI.
    pub attach_summaries: Vec<AttachSummary>,
    /// Target row ids created by this import (live runs only). An opt-in
    /// database quality job can scan these after the import completes.
    pub created_target_ids: Vec<i32>,
    /// Existing target ids that gained frames (live runs only).
    pub attached_target_ids: Vec<i32>,
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

    // Merge phase: attach frames to targets that already exist (matched by
    // name, then coordinates). Only what's left falls through to the
    // ground-zero grouping below. This is what keeps a re-import against a
    // real scheduler database from duplicating its structure.
    let mut fresh: Vec<FrameMeta> = Vec::new();
    if options.attach_existing {
        let existing_targets = load_existing_targets(&tx)?;
        let mut attach: HashMap<usize, Vec<FrameMeta>> = HashMap::new();
        let mut matched_by: HashMap<usize, &'static str> = HashMap::new();
        for frame in lights {
            match match_existing_target(&frame, &existing_targets, options.match_radius_deg) {
                Some((idx, how)) => {
                    attach.entry(idx).or_default().push(frame);
                    matched_by.entry(idx).or_insert(how);
                }
                None => fresh.push(frame),
            }
        }

        // Deterministic order for summaries and tests.
        let mut attach: Vec<(usize, Vec<FrameMeta>)> = attach.into_iter().collect();
        attach.sort_by_key(|(idx, _)| *idx);
        for (idx, frames) in attach {
            let target = &existing_targets[idx];
            attach_frames_to_target(&tx, target, &frames, &mut outcome)?;
            outcome.attached += frames.len();
            outcome.attached_target_ids.push(target.id as i32);
            outcome.attach_summaries.push(AttachSummary {
                project: target.project_name.clone(),
                target: target.name.clone(),
                frames: frames.len(),
                matched_by: matched_by.get(&idx).copied().unwrap_or("name").to_string(),
            });
        }
    } else {
        fresh = lights;
    }
    let lights = fresh;

    let plan = build_plan(&lights, options.time_gap_days);
    // Only resolve (or create) an import profile when new structure is
    // actually being created — a fully-attached import into a multi-profile
    // database must not fail on profile ambiguity or add a profile row.
    let profile_id = if plan.projects.is_empty() {
        String::new()
    } else {
        resolve_profile(&tx, options)?
    };
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
        // rows that don't exist). Attached ids reference REAL targets, but
        // the frames weren't kept, so clear those too.
        outcome.created_target_ids.clear();
        outcome.attached_target_ids.clear();
    } else {
        tx.commit().context("committing import transaction")?;
    }
    Ok(outcome)
}

/// Remove every project this importer created (recognized by the
/// `Imported by PSF Guard` description marker) together with its rule
/// weights, targets, exposure plans, acquired images, and their thumbnail
/// blobs. Recovery hatch for an import that should not have happened —
/// imported rows only ever land in marker projects (attached frames go to
/// pre-existing projects and are deliberately NOT touched).
///
/// Returns (projects, targets, plans, images) removed.
pub fn remove_imported(
    conn: &mut Connection,
    dry_run: bool,
) -> Result<(usize, usize, usize, usize)> {
    let tx = conn.transaction()?;
    let project_ids: Vec<i64> = {
        let mut stmt =
            tx.prepare("SELECT Id FROM project WHERE description LIKE 'Imported by PSF Guard%'")?;
        stmt.query_map([], |row| row.get(0))?
            .collect::<Result<_, _>>()?
    };
    if project_ids.is_empty() {
        tx.rollback()?;
        return Ok((0, 0, 0, 0));
    }
    let placeholders = vec!["?"; project_ids.len()].join(",");
    let params: Vec<&dyn rusqlite::ToSql> = project_ids
        .iter()
        .map(|id| id as &dyn rusqlite::ToSql)
        .collect();

    let images = tx.execute(
        &format!(
            "DELETE FROM imagedata WHERE acquiredimageid IN
             (SELECT Id FROM acquiredimage WHERE projectId IN ({placeholders}))"
        ),
        params.as_slice(),
    )?;
    let _ = images; // thumbnail rows; the acquiredimage count below is reported
    let images = tx.execute(
        &format!("DELETE FROM acquiredimage WHERE projectId IN ({placeholders})"),
        params.as_slice(),
    )?;
    let plans = tx.execute(
        &format!(
            "DELETE FROM exposureplan WHERE targetid IN
             (SELECT Id FROM target WHERE projectid IN ({placeholders}))"
        ),
        params.as_slice(),
    )?;
    let targets = tx.execute(
        &format!("DELETE FROM target WHERE projectid IN ({placeholders})"),
        params.as_slice(),
    )?;
    tx.execute(
        &format!("DELETE FROM ruleweight WHERE projectid IN ({placeholders})"),
        params.as_slice(),
    )?;
    let projects = tx.execute(
        &format!("DELETE FROM project WHERE Id IN ({placeholders})"),
        params.as_slice(),
    )?;

    if dry_run {
        tx.rollback()?;
    } else {
        tx.commit()?;
    }
    Ok((projects, targets, plans, images))
}

/// An existing target row, loaded once per import for merge matching.
#[derive(Debug)]
struct ExistingTarget {
    id: i64,
    project_id: i64,
    name: String,
    /// Converted from the DB's decimal hours to degrees at load.
    ra_deg: Option<f64>,
    dec_deg: Option<f64>,
    profile_id: String,
    project_name: String,
}

fn load_existing_targets(conn: &Connection) -> Result<Vec<ExistingTarget>> {
    let mut stmt = conn.prepare(
        "SELECT t.Id, t.projectid, t.name, t.ra, t.dec, p.profileId, p.name
         FROM target t JOIN project p ON p.Id = t.projectid
         ORDER BY t.Id",
    )?;
    let targets = stmt
        .query_map([], |row| {
            Ok(ExistingTarget {
                id: row.get(0)?,
                project_id: row.get(1)?,
                name: row.get::<_, String>(2)?,
                ra_deg: row.get::<_, Option<f64>>(3)?.map(|hours| hours * 15.0),
                dec_deg: row.get(4)?,
                profile_id: row.get(5)?,
                project_name: row.get(6)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(targets)
}

/// Angular separation in degrees (small-angle approximation — fine at the
/// sub-degree radii used for target matching).
fn angular_separation_deg(ra1: f64, dec1: f64, ra2: f64, dec2: f64) -> f64 {
    let dra = ((ra1 - ra2 + 180.0).rem_euclid(360.0) - 180.0)
        * dec1
            .to_radians()
            .cos()
            .max(dec2.to_radians().cos().min(1.0));
    let ddec = dec1 - dec2;
    (dra * dra + ddec * ddec).sqrt()
}

/// Find the existing target a frame belongs to: exact (case-insensitive)
/// OBJECT-name match wins; otherwise the nearest target whose coordinates
/// are within `radius_deg`. Returns the index and how the match was made.
fn match_existing_target(
    frame: &FrameMeta,
    targets: &[ExistingTarget],
    radius_deg: f64,
) -> Option<(usize, &'static str)> {
    if let Some(object) = frame
        .object
        .as_deref()
        .map(str::trim)
        .filter(|o| !o.is_empty())
    {
        // Multiple targets can share a name (mosaic panels); pick the one
        // nearest the frame, falling back to the first.
        let mut named: Vec<usize> = targets
            .iter()
            .enumerate()
            .filter(|(_, t)| t.name.trim().eq_ignore_ascii_case(object))
            .map(|(i, _)| i)
            .collect();
        if !named.is_empty() {
            if let (Some(ra), Some(dec)) = (frame.ra_deg, frame.dec_deg) {
                named.sort_by(|&a, &b| {
                    let sep = |i: usize| match (targets[i].ra_deg, targets[i].dec_deg) {
                        (Some(tra), Some(tdec)) => angular_separation_deg(ra, dec, tra, tdec),
                        _ => f64::MAX,
                    };
                    sep(a).total_cmp(&sep(b))
                });
            }
            return Some((named[0], "name"));
        }
    }

    let (ra, dec) = (frame.ra_deg?, frame.dec_deg?);
    targets
        .iter()
        .enumerate()
        .filter_map(|(i, t)| {
            let (tra, tdec) = (t.ra_deg?, t.dec_deg?);
            let sep = angular_separation_deg(ra, dec, tra, tdec);
            (sep <= radius_deg).then_some((i, sep))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(i, _)| (i, "coordinates"))
}

/// Insert frames into an EXISTING target: reuse (or create) the profile's
/// exposure template, reuse a matching exposure plan on the target (bumping
/// its acquired count) or add one, and land the images under the target's
/// own project and profile.
fn attach_frames_to_target(
    conn: &Connection,
    target: &ExistingTarget,
    frames: &[FrameMeta],
    outcome: &mut ImportOutcome,
) -> Result<()> {
    use rusqlite::OptionalExtension;

    // Group by template + exposure, exactly like the ground-zero path.
    let mut groups: std::collections::BTreeMap<(TemplateKey, i64), Vec<&FrameMeta>> =
        std::collections::BTreeMap::new();
    for frame in frames {
        let key = TemplateKey::of(frame);
        let exp_ms = frame
            .exposure_s
            .map(|e| (e * 1000.0).round() as i64)
            .unwrap_or(0);
        groups.entry((key, exp_ms)).or_default().push(frame);
    }

    for ((key, exp_ms), group) in groups {
        let exposure_s = exp_ms as f64 / 1000.0;
        let template_id =
            find_or_create_template(conn, &key, &target.profile_id, exposure_s, outcome)?;

        // Reuse the target's own plan for this template + exposure length.
        let existing_plan: Option<i64> = conn
            .query_row(
                "SELECT Id FROM exposureplan
                 WHERE targetid = ?1 AND exposureTemplateId = ?2
                   AND ABS(IFNULL(exposure, -1) - ?3) < 0.001",
                params![
                    target.id,
                    template_id,
                    if exposure_s > 0.0 { exposure_s } else { -1.0 }
                ],
                |row| row.get(0),
            )
            .optional()?;
        let plan_id = match existing_plan {
            Some(id) => {
                conn.execute(
                    "UPDATE exposureplan SET acquired = IFNULL(acquired, 0) + ?1 WHERE Id = ?2",
                    params![group.len() as i64, id],
                )?;
                id
            }
            None => {
                let id = insert_exposure_plan(
                    conn,
                    &target.profile_id,
                    exposure_s,
                    group.len(),
                    target.id,
                    template_id,
                )?;
                outcome.plans_created += 1;
                id
            }
        };

        for frame in group {
            insert_acquired_image(
                conn,
                frame,
                target.project_id,
                target.id,
                plan_id,
                &target.profile_id,
            )?;
            outcome.imported += 1;
        }
    }
    Ok(())
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
        let default_exposure = frames_per_exposure
            .iter()
            .filter(|((k, _), _)| *k == key)
            .max_by_key(|(_, count)| **count)
            .map(|((_, exp_ms), _)| *exp_ms as f64 / 1000.0)
            .filter(|e| *e > 0.0)
            .unwrap_or(60.0);
        let id = find_or_create_template(conn, &key, profile_id, default_exposure, outcome)?;
        ids.insert(key, id);
    }
    Ok(ids)
}

/// Reuse the profile's template matching this key, or insert one with the
/// plugin's constructor defaults. Shared by the ground-zero and the
/// attach-to-existing paths.
fn find_or_create_template(
    conn: &Connection,
    key: &TemplateKey,
    profile_id: &str,
    default_exposure: f64,
    outcome: &mut ImportOutcome,
) -> Result<i64> {
    use rusqlite::OptionalExtension;

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
        .optional()?;
    if let Some(id) = existing {
        outcome.templates_reused += 1;
        return Ok(id);
    }

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
            if default_exposure > 0.0 {
                default_exposure
            } else {
                60.0
            },
            new_guid(),
        ],
    )?;
    outcome.templates_created += 1;
    Ok(conn.last_insert_rowid())
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
    if outcome.attached > 0 {
        println!("    → to existing:  {}", outcome.attached);
    }
    println!(
        "  Projects: {}  Targets: {}  Exposure plans: {}  Templates: {} new / {} reused",
        outcome.projects_created,
        outcome.targets_created,
        outcome.plans_created,
        outcome.templates_created,
        outcome.templates_reused,
    );
    if !outcome.profile_id.is_empty() {
        println!("  Profile: {}", outcome.profile_id);
    }
    for attach in &outcome.attach_summaries {
        println!(
            "    ↳ existing {} / {} — +{} frame(s) (matched by {})",
            attach.project, attach.target, attach.frames, attach.matched_by
        );
    }
    for summary in &outcome.project_summaries {
        println!(
            "    NEW {} — {} target(s), {} frame(s)",
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
        assert_eq!(outcome.projects_created, 2);
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

        // Rule weights exist for both projects.
        let weights: i64 = conn
            .query_row("SELECT COUNT(*) FROM ruleweight", [], |row| row.get(0))
            .unwrap();
        assert_eq!(weights, 16);

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

    /// Seed an existing project + target (profile `p1`) the way a real TS
    /// database would have them: RA stored in decimal hours.
    fn seed_existing_target(conn: &Connection, name: &str, ra_deg: f64, dec_deg: f64) {
        conn.execute_batch(&format!(
            "INSERT INTO profilepreference (profileId, guid) VALUES ('p1', 'pp-guid');
             INSERT INTO project (Id, profileId, name, description, guid)
             VALUES (10, 'p1', 'Existing Project', 'real project', 'proj-guid');
             INSERT INTO target (Id, name, active, ra, dec, epochcode, projectid, guid)
             VALUES (20, '{name}', 1, {ra_hours}, {dec_deg}, 2, 10, 'tgt-guid');",
            name = name,
            ra_hours = ra_deg / 15.0,
            dec_deg = dec_deg,
        ))
        .unwrap();
    }

    #[test]
    fn attaches_by_object_name_to_existing_target() {
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);

        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions::default(),
        )
        .unwrap();

        assert_eq!(outcome.attached, 1);
        assert_eq!(outcome.projects_created, 0, "must not synthesize a project");
        assert_eq!(outcome.targets_created, 0);
        assert_eq!(outcome.attach_summaries.len(), 1);
        assert_eq!(outcome.attach_summaries[0].matched_by, "name");
        assert_eq!(outcome.attach_summaries[0].target, "M31");
        assert_eq!(outcome.attached_target_ids, vec![20]);

        let (project_id, target_id, profile): (i64, i64, String) = conn
            .query_row(
                "SELECT projectId, targetId, profileId FROM acquiredimage",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((project_id, target_id, profile.as_str()), (10, 20, "p1"));
        // The plan landed on the existing target under its profile.
        let (plan_target, plan_profile): (i64, String) = conn
            .query_row("SELECT targetid, profileId FROM exposureplan", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!((plan_target, plan_profile.as_str()), (20, "p1"));
    }

    #[test]
    fn attaches_by_coordinates_when_name_differs() {
        let mut conn = fresh_conn();
        // Existing target named differently but at the frame's coordinates.
        seed_existing_target(&conn, "Andromeda Panel 1", 10.68, 41.27);

        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(outcome.attached, 1);
        assert_eq!(outcome.projects_created, 0);
        assert_eq!(outcome.attach_summaries[0].matched_by, "coordinates");
    }

    #[test]
    fn distant_unmatched_frames_still_create_new_structure() {
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);

        // Unknown object 90 degrees away: nothing to attach to.
        let mut frame = light("Unknown Nebula", "Ha", 1_000);
        frame.ra_deg = Some(180.0);
        frame.dec_deg = Some(-30.0);
        let outcome = import_frames(&mut conn, vec![frame], &ImportOptions::default()).unwrap();
        assert_eq!(outcome.attached, 0);
        assert_eq!(outcome.projects_created, 1);
        assert_eq!(outcome.targets_created, 1);
    }

    #[test]
    fn attach_reuses_matching_plan_and_bumps_acquired() {
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);
        conn.execute_batch(
            "INSERT INTO exposuretemplate (Id, profileId, name, filtername, gain, offset, bin, readoutmode, guid)
             VALUES (30, 'p1', 'Ha tmpl', 'Ha', 100, 30, 1, -1, 'tmpl-guid');
             INSERT INTO exposureplan (Id, profileId, exposure, desired, acquired, accepted, targetid, exposureTemplateId, enabled, guid)
             VALUES (40, 'p1', 300.0, 50, 7, 3, 20, 30, 1, 'plan-guid');",
        )
        .unwrap();

        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000), light("M31", "Ha", 2_000)],
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(outcome.attached, 2);
        assert_eq!(outcome.plans_created, 0, "existing plan reused");
        assert_eq!(outcome.templates_created, 0);
        assert_eq!(outcome.templates_reused, 1);

        let (acquired, desired, plan_count): (i64, i64, i64) = conn
            .query_row(
                "SELECT acquired, desired, (SELECT COUNT(*) FROM exposureplan) FROM exposureplan WHERE Id = 40",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        assert_eq!((acquired, desired, plan_count), (9, 50, 1));
        let exposure_ids: Vec<i64> = conn
            .prepare("SELECT DISTINCT exposureId FROM acquiredimage")
            .unwrap()
            .query_map([], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(exposure_ids, vec![40]);
    }

    #[test]
    fn no_attach_forces_ground_zero() {
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);
        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions {
                attach_existing: false,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.attached, 0);
        assert_eq!(outcome.projects_created, 1);
    }

    #[test]
    fn fully_attached_import_ignores_profile_ambiguity() {
        // A real DB with several profiles used to make import bail with
        // "pass --profile-id" even when every frame attaches to existing
        // targets and no new structure is needed.
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);
        ensure_profile_preference(&conn, "p2").unwrap();

        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions::default(),
        )
        .unwrap();
        assert_eq!(outcome.attached, 1);
        assert_eq!(outcome.profile_id, "", "no import profile resolved");
    }

    #[test]
    fn attached_dry_run_writes_nothing_and_clears_ids() {
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);
        let outcome = import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000)],
            &ImportOptions {
                dry_run: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(outcome.attached, 1, "preview still reports the attach");
        assert!(outcome.attached_target_ids.is_empty());
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM acquiredimage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[test]
    fn remove_imported_deletes_only_marker_projects() {
        let mut conn = fresh_conn();
        seed_existing_target(&conn, "M31", 10.68, 41.27);

        // One attached frame (goes to the pre-existing project) and one
        // ground-zero frame (creates a marker project).
        let mut far = light("Far Nebula", "Ha", 2_000);
        far.ra_deg = Some(200.0);
        far.dec_deg = Some(10.0);
        import_frames(
            &mut conn,
            vec![light("M31", "Ha", 1_000), far],
            &ImportOptions::default(),
        )
        .unwrap();

        // Dry run counts, changes nothing.
        let (p, t, _pl, i) = remove_imported(&mut conn, true).unwrap();
        assert_eq!((p, t, i), (1, 1, 1));
        let projects: i64 = conn
            .query_row("SELECT COUNT(*) FROM project", [], |r| r.get(0))
            .unwrap();
        assert_eq!(projects, 2);

        // Live run removes only the marker project and its rows.
        let (p, t, _pl, i) = remove_imported(&mut conn, false).unwrap();
        assert_eq!((p, t, i), (1, 1, 1));
        let (projects, targets, images): (i64, i64, i64) = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM project), (SELECT COUNT(*) FROM target),
                        (SELECT COUNT(*) FROM acquiredimage)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .unwrap();
        // Pre-existing project/target survive, as does the ATTACHED frame.
        assert_eq!((projects, targets, images), (1, 1, 1));
        let name: String = conn
            .query_row("SELECT name FROM project", [], |r| r.get(0))
            .unwrap();
        assert_eq!(name, "Existing Project");
    }
}
