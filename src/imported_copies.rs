//! Lights catalogued twice under one file name (#1100): once as N.I.N.A.'s
//! Target Scheduler recorded them, and once by a PSF Guard import of the
//! same file. A merge of two catalogs leaves these when one catalog has
//! Target Scheduler's records and the other imported the capture folder:
//! projects and targets match only by GUID, so the frames arrive in a
//! second project and target, and the sync's adoption of a row PSF Guard
//! minted needed the same target until it learned to look across targets.
//!
//! - [`plan`] groups the lights that are the same exposure under the same
//!   file name: the same capture time, exposure and filter, whatever their
//!   profile, project or target. Each group has one light Target Scheduler
//!   recorded, which stays, and the copies PSF Guard minted, which go.
//! - It also lists the projects and targets an import created that will
//!   have no lights left, and that nothing else uses: those go too.
//! - [`fold`] does both in one transaction, and refuses when the catalog no
//!   longer matches the preview's digest.
//!
//! The light that stays keeps its project, target, exposure plan, GUID and
//! file name, so Target Scheduler's counts do not drop. It takes a copy's
//! grade when it has none. Lights with different grades are left alone.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{params, Connection, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::commands::import::IMPORTED_PROJECT_MARKER;
use crate::commands::project_removal;
use crate::frame_derivatives::identities_match;
use crate::renamed_lights::{
    base_name, has_column, lights, now, table_exists, Light, LightRemoval, Records,
};

/// One exposure catalogued more than once: the light Target Scheduler
/// recorded, and the copies an import made of its file.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedCopies {
    /// The light Target Scheduler recorded. It stays as it is.
    pub kept: Light,
    /// The lights PSF Guard catalogued from the same file. They are removed.
    pub copies: Vec<Light>,
    /// Whether the kept light takes a copy's grade: it had none, and a copy
    /// has one.
    pub takes_grade: bool,
    /// The copy whose grade it takes.
    #[serde(skip)]
    grade_from: Option<usize>,
}

impl ImportedCopies {
    /// Whether the kept light takes this copy's grade.
    pub fn grade_from_copy(&self, copy: &Light) -> bool {
        self.grade_from
            .is_some_and(|index| self.copies[index].id == copy.id)
    }
}

/// A group left alone, and why.
#[derive(Debug, Clone, Serialize)]
pub struct HeldCopies {
    pub kept: Light,
    pub copies: Vec<Light>,
    pub reason: String,
}

/// A project an import created that has no lights left. It is deleted with
/// its targets and their exposure plans.
#[derive(Debug, Clone, Serialize)]
pub struct EmptyProject {
    pub id: i64,
    pub name: String,
    pub targets: Vec<String>,
    pub exposure_plans: usize,
}

/// A target with no lights left, in a project an import created that keeps
/// other lights. It is deleted with its exposure plans.
#[derive(Debug, Clone, Serialize)]
pub struct EmptyTarget {
    pub id: i64,
    pub name: String,
    pub project_id: i64,
    pub project: String,
    pub exposure_plans: usize,
}

/// A project an import created that has no lights left, kept, and why.
#[derive(Debug, Clone, Serialize)]
pub struct KeptProject {
    pub id: i64,
    pub name: String,
    pub reason: String,
}

/// What [`fold`] would do.
#[derive(Debug, Clone, Default, Serialize)]
pub struct FoldPlan {
    pub groups: Vec<ImportedCopies>,
    pub held: Vec<HeldCopies>,
    pub projects: Vec<EmptyProject>,
    pub targets: Vec<EmptyTarget>,
    pub kept_projects: Vec<KeptProject>,
    /// Changes when anything the fold would touch does.
    pub digest: String,
}

impl FoldPlan {
    /// Whether applying it would change anything.
    pub fn is_empty(&self) -> bool {
        self.groups.is_empty() && self.projects.is_empty() && self.targets.is_empty()
    }

    /// The lights a fold removes.
    pub fn removed(&self) -> impl Iterator<Item = &Light> {
        self.groups.iter().flat_map(|group| &group.copies)
    }
}

/// What else keeps a project or target an import created.
#[derive(Debug, Clone, Default)]
pub struct Keepers {
    /// Light GUIDs a collaboration report names.
    pub reported: BTreeSet<String>,
    /// Project GUIDs, lower case, that a Director plan links.
    pub planned_projects: BTreeSet<String>,
}

/// The light's capture fields without its target's name: the two rows
/// sit in targets named differently.
fn same_exposure(left: &Light, right: &Light) -> bool {
    let mut left = left.identity.clone();
    let mut right = right.identity.clone();
    left.target = None;
    right.target = None;
    identities_match(&left, &right)
}

/// Group lights of one file name into sets of the same exposure.
fn clusters(all: &[Light], members: &[usize]) -> Vec<Vec<usize>> {
    let mut parent: Vec<usize> = (0..members.len()).collect();
    fn root(parent: &mut [usize], mut index: usize) -> usize {
        while parent[index] != index {
            parent[index] = parent[parent[index]];
            index = parent[index];
        }
        index
    }
    for left in 0..members.len() {
        for right in left + 1..members.len() {
            if same_exposure(&all[members[left]], &all[members[right]]) {
                let (a, b) = (root(&mut parent, left), root(&mut parent, right));
                if a != b {
                    parent[b] = a;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (index, member) in members.iter().enumerate() {
        let top = root(&mut parent, index);
        groups.entry(top).or_default().push(*member);
    }
    groups
        .into_values()
        .filter(|group| group.len() > 1)
        .collect()
}

/// The groups in the catalog, and the projects and targets that would be
/// left empty.
pub fn plan(conn: &Connection, keepers: &Keepers) -> Result<FoldPlan> {
    let mut plan = FoldPlan::default();
    let all = lights(conn)?;
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, light) in all.iter().enumerate() {
        by_name
            .entry(base_name(&light.file_name).to_lowercase())
            .or_default()
            .push(index);
    }
    let records = Records::read(conn)?;
    let mut buckets: Vec<Vec<usize>> = by_name
        .into_values()
        .filter(|bucket| bucket.len() > 1)
        .collect();
    buckets.sort();
    for bucket in buckets {
        for group in clusters(&all, &bucket) {
            let (recorded, minted): (Vec<usize>, Vec<usize>) = group
                .iter()
                .copied()
                .partition(|index| all[*index].recorded_by_nina);
            // Two imports of one file, or two Target Scheduler records of
            // it, are not this repair's to settle.
            let (Some(&keeper), false) = (
                recorded.iter().min_by_key(|i| all[**i].id),
                minted.is_empty(),
            ) else {
                continue;
            };
            let kept = &all[keeper];
            let mut copies: Vec<&Light> = minted.iter().map(|index| &all[*index]).collect();
            copies.sort_by_key(|light| light.id);
            let hold = |reason: String| HeldCopies {
                kept: kept.clone(),
                copies: copies.iter().copied().cloned().collect(),
                reason,
            };
            if recorded.len() > 1 {
                plan.held.push(hold(
                    "Target Scheduler recorded this exposure more than once".into(),
                ));
                continue;
            }
            if !copies.iter().all(|copy| same_exposure(kept, copy)) {
                plan.held.push(hold(
                    "it matches lights that are not copies of one another".into(),
                ));
                continue;
            }
            if let Some(reason) = records.keeps_own_file(kept) {
                plan.held.push(hold(format!(
                    "Target Scheduler's light must stay as it is: {reason}"
                )));
                continue;
            }
            if let Some(reason) = copies
                .iter()
                .find_map(|copy| records.must_stay(copy, &keepers.reported))
            {
                plan.held
                    .push(hold(format!("an imported copy must stay: {reason}")));
                continue;
            }
            let grades: BTreeSet<i64> = std::iter::once(kept)
                .chain(copies.iter().copied())
                .map(|light| light.grade)
                .filter(|grade| *grade != 0)
                .collect();
            if grades.len() > 1 {
                plan.held.push(hold(
                    "the lights have different grades; grade them to match".into(),
                ));
                continue;
            }
            let grade_from = (kept.grade == 0)
                .then(|| copies.iter().position(|copy| copy.grade != 0))
                .flatten();
            plan.groups.push(ImportedCopies {
                kept: kept.clone(),
                copies: copies.into_iter().cloned().collect(),
                takes_grade: grade_from.is_some(),
                grade_from,
            });
        }
    }
    plan.groups.sort_by_key(|group| group.kept.id);
    plan.held.sort_by_key(|held| held.kept.id);
    let removed: HashSet<i64> = plan.removed().map(|light| light.id).collect();
    empty_imports(conn, &removed, keepers, &mut plan)?;
    plan.digest = digest(conn, &plan)?;
    Ok(plan)
}

/// A light's place, for counting what a project or target keeps.
struct Place {
    id: i64,
    project_id: Option<i64>,
    target_id: Option<i64>,
}

fn guids_in(conn: &Connection, table: &str, column: &str) -> Result<HashSet<String>> {
    if !has_column(conn, table, column) {
        return Ok(HashSet::new());
    }
    let mut statement = conn.prepare(&format!(
        "SELECT DISTINCT lower(\"{column}\") FROM \"{table}\" WHERE \"{column}\" IS NOT NULL"
    ))?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The projects and targets an import created that a merge leaves with no
/// lights: this one, which removes the plan's copies, or an earlier one
/// recorded in `psf_guard_merged_light`. A project goes whole when each of
/// its targets is such a target; otherwise only those targets go. A project
/// Target Scheduler has active, or that Planning uses, stays whole. An
/// empty target no merge took a light from stays: nothing here made it so.
fn empty_imports(
    conn: &Connection,
    removed: &HashSet<i64>,
    keepers: &Keepers,
    plan: &mut FoldPlan,
) -> Result<()> {
    if !has_column(conn, "project", "description") || !table_exists(conn, "target") {
        return Ok(());
    }
    let project_guid = if has_column(conn, "project", "guid") {
        "guid"
    } else {
        "NULL"
    };
    let state = if has_column(conn, "project", "state") {
        "state"
    } else {
        "0"
    };
    let mut statement = conn.prepare(&format!(
        "SELECT Id, name, {project_guid}, {state} FROM project
         WHERE description LIKE ?1 ORDER BY Id"
    ))?;
    let projects: Vec<(i64, String, Option<String>, Option<i64>)> = statement
        .query_map([format!("{IMPORTED_PROJECT_MARKER}%")], |row| {
            Ok((
                row.get(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                row.get(2)?,
                row.get(3)?,
            ))
        })?
        .collect::<rusqlite::Result<_>>()?;
    if projects.is_empty() {
        return Ok(());
    }
    let places: Vec<Place> = {
        let mut statement = conn.prepare("SELECT Id, projectId, targetId FROM acquiredimage")?;
        let rows = statement.query_map([], |row| {
            Ok(Place {
                id: row.get(0)?,
                project_id: row.get(1)?,
                target_id: row.get(2)?,
            })
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .filter(|place| !removed.contains(&place.id))
            .collect()
    };
    let mut lights_in_target: HashMap<i64, usize> = HashMap::new();
    let mut lights_in_project: HashMap<i64, usize> = HashMap::new();
    for place in &places {
        if let Some(target) = place.target_id {
            *lights_in_target.entry(target).or_default() += 1;
        }
        if let Some(project) = place.project_id {
            *lights_in_project.entry(project).or_default() += 1;
        }
    }
    let target_guid = if has_column(conn, "target", "guid") {
        "guid"
    } else {
        "NULL"
    };
    let mut targets_of = conn.prepare(&format!(
        "SELECT Id, name, {target_guid} FROM target WHERE projectid = ?1 ORDER BY Id"
    ))?;
    let has_plans = table_exists(conn, "exposureplan");
    let plans_of = |target: i64| -> Result<usize> {
        if !has_plans {
            return Ok(0);
        }
        Ok(conn.query_row(
            "SELECT COUNT(*) FROM exposureplan WHERE targetid = ?1",
            [target],
            |row| row.get::<_, i64>(0),
        )? as usize)
    };
    // Planning's records in this database.
    let planned_projects = guids_in(conn, "psf_guard_director_project", "project_guid")?;
    let planned_targets = guids_in(conn, "psf_guard_director_target", "target_guid")?;
    // Targets a merge took a light from, now or before: an empty target
    // that never lost one to a merge is left as it is.
    let merged_from = guids_in(conn, "psf_guard_merged_light", "target_guid")?;
    let losing: HashSet<i64> = plan.removed().filter_map(|light| light.target_id).collect();
    for (project_id, name, guid, state) in projects {
        let targets: Vec<(i64, String, Option<String>)> = targets_of
            .query_map([project_id], |row| {
                Ok((
                    row.get(0)?,
                    row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    row.get(2)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        let merged = |id: &i64, guid: &Option<String>| {
            losing.contains(id)
                || guid
                    .as_ref()
                    .is_some_and(|guid| merged_from.contains(&guid.trim().to_lowercase()))
        };
        let empty: Vec<&(i64, String, Option<String>)> = targets
            .iter()
            .filter(|(id, _, guid)| {
                lights_in_target.get(id).copied().unwrap_or(0) == 0 && merged(id, guid)
            })
            .collect();
        let whole = !targets.is_empty()
            && empty.len() == targets.len()
            && lights_in_project.get(&project_id).copied().unwrap_or(0) == 0;
        if empty.is_empty() {
            continue;
        }
        let guid = guid.map(|guid| guid.trim().to_lowercase());
        let keep = |reason: &str| KeptProject {
            id: project_id,
            name: name.clone(),
            reason: reason.into(),
        };
        if guid.as_ref().is_some_and(|guid| {
            keepers.planned_projects.contains(guid) || planned_projects.contains(guid)
        }) {
            plan.kept_projects.push(keep("a Director plan uses it"));
            continue;
        }
        if state == Some(1) {
            plan.kept_projects
                .push(keep("it is active in Target Scheduler"));
            continue;
        }
        let planned = |target_guid: &Option<String>| {
            target_guid
                .as_ref()
                .is_some_and(|guid| planned_targets.contains(&guid.trim().to_lowercase()))
        };
        if whole && !targets.iter().any(|(_, _, guid)| planned(guid)) {
            let mut exposure_plans = 0;
            for (id, _, _) in &targets {
                exposure_plans += plans_of(*id)?;
            }
            plan.projects.push(EmptyProject {
                id: project_id,
                name,
                targets: targets.iter().map(|(_, name, _)| name.clone()).collect(),
                exposure_plans,
            });
            continue;
        }
        for (id, target, target_guid) in empty {
            if planned(target_guid) {
                continue;
            }
            plan.targets.push(EmptyTarget {
                id: *id,
                name: target.clone(),
                project_id,
                project: name.clone(),
                exposure_plans: plans_of(*id)?,
            });
        }
    }
    Ok(())
}

/// What the plan touches, hashed: each light's row, grade and plan, and each
/// project's and target's rows. Exposure plans are listed by Id.
fn digest(conn: &Connection, plan: &FoldPlan) -> Result<String> {
    let mut hasher = Sha256::new();
    let mut light = |label: u8, light: &Light| {
        hasher.update([label]);
        hasher.update(light.id.to_le_bytes());
        hasher.update(light.guid.as_bytes());
        hasher.update([0]);
        hasher.update(light.grade.to_le_bytes());
        hasher.update(light.exposure_id.unwrap_or(-1).to_le_bytes());
        hasher.update(light.target_id.unwrap_or(-1).to_le_bytes());
    };
    for group in &plan.groups {
        light(b'G', &group.kept);
        for copy in &group.copies {
            light(b'C', copy);
        }
    }
    for held in &plan.held {
        light(b'H', &held.kept);
        for copy in &held.copies {
            light(b'c', copy);
        }
    }
    let has_plans = table_exists(conn, "exposureplan");
    let plan_ids = |target: i64| -> Result<Vec<i64>> {
        if !has_plans {
            return Ok(Vec::new());
        }
        let mut statement =
            conn.prepare_cached("SELECT Id FROM exposureplan WHERE targetid = ?1 ORDER BY Id")?;
        let rows = statement.query_map([target], |row| row.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    };
    let mut ids: Vec<(u8, i64, Vec<i64>)> = Vec::new();
    for project in &plan.projects {
        let mut statement =
            conn.prepare_cached("SELECT Id FROM target WHERE projectid = ?1 ORDER BY Id")?;
        let targets: Vec<i64> = statement
            .query_map([project.id], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        ids.push((b'P', project.id, targets.clone()));
        for target in targets {
            ids.push((b'E', target, plan_ids(target)?));
        }
    }
    for target in &plan.targets {
        ids.push((b'T', target.id, plan_ids(target.id)?));
    }
    for (label, id, children) in ids {
        hasher.update([label]);
        hasher.update(id.to_le_bytes());
        for child in children {
            hasher.update(child.to_le_bytes());
        }
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Delete a target and what belongs to it, as a project deletion does for
/// each of its targets. The caller has checked it has no lights.
fn delete_target(tx: &Connection, target_id: i64) -> Result<()> {
    if has_column(tx, "psf_guard_removed_image", "target_id") {
        tx.execute(
            "UPDATE psf_guard_removed_image SET target_id = NULL, exposure_id = NULL
             WHERE target_id = ?1",
            [target_id],
        )?;
    }
    if has_column(tx, "target", "guid")
        && has_column(tx, "psf_guard_scheduler_flat_history", "target_guid")
    {
        tx.execute(
            "DELETE FROM psf_guard_scheduler_flat_history
             WHERE target_guid = (SELECT guid FROM target WHERE Id = ?1)",
            [target_id],
        )?;
    }
    for (table, column) in [
        ("overrideexposureorderitem", "targetid"),
        ("filtercadenceitem", "targetid"),
        ("flathistory", "targetId"),
        ("exposureplan", "targetid"),
    ] {
        if has_column(tx, table, column) {
            tx.execute(
                &format!("DELETE FROM \"{table}\" WHERE \"{column}\" = ?1"),
                [target_id],
            )?;
        }
    }
    tx.execute("DELETE FROM target WHERE Id = ?1", [target_id])?;
    Ok(())
}

/// How [`fold`] ended.
#[derive(Debug, Clone)]
pub enum FoldOutcome {
    /// It did what the plan lists.
    Folded(FoldPlan),
    /// The catalog changed since the preview, so nothing changed. The plan
    /// is the new preview.
    Changed(FoldPlan),
}

/// Fold each group into its kept light and delete the empty projects and
/// targets, in one transaction. With `digest`, refuse when the plan made
/// inside the transaction has another one.
pub fn fold(conn: &mut Connection, digest: Option<&str>, keepers: &Keepers) -> Result<FoldOutcome> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let plan = plan(&tx, keepers)?;
    if digest.is_some_and(|digest| digest != plan.digest) {
        return Ok(FoldOutcome::Changed(plan));
    }
    if plan.is_empty() {
        return Ok(FoldOutcome::Folded(plan));
    }
    let now = now();
    let removal = LightRemoval::new(&tx)?;
    let has_plans = has_column(&tx, "exposureplan", "accepted");
    let dates_rejects = table_exists(&tx, "psf_guard_rejected_at");
    for group in &plan.groups {
        let kept = &group.kept;
        if let Some(copy) = group.grade_from.map(|index| &group.copies[index]) {
            tx.execute(
                "UPDATE acquiredimage SET gradingStatus = ?1, rejectreason = ?2 WHERE Id = ?3",
                params![copy.grade, copy.reject_reason, kept.id],
            )?;
            // A reject keeps the date it was rejected on.
            if copy.grade == 2 && dates_rejects {
                tx.execute(
                    "UPDATE OR IGNORE psf_guard_rejected_at SET acquired_image_guid = ?1
                     WHERE acquired_image_guid = ?2",
                    params![kept.guid, copy.guid],
                )?;
            }
            // Target Scheduler's plan counts one more accepted frame. Its
            // counters are moved by one, not counted again, so a counter
            // that already differs from the catalog's lights never drops.
            if copy.grade == 1
                && has_plans
                && let Some(exposure_id) = kept.exposure_id
            {
                tx.execute(
                    "UPDATE exposureplan SET accepted = accepted + 1 WHERE Id = ?1",
                    [exposure_id],
                )?;
            }
        }
        for copy in &group.copies {
            removal.remove(&tx, kept, copy, now)?;
            removal.uncount(&tx, copy, copy.grade == 1)?;
        }
    }
    for target in &plan.targets {
        delete_target(&tx, target.id)?;
    }
    for project in &plan.projects {
        let Some(removal) = project_removal::plan(&tx, project.id, &keepers.reported)? else {
            continue;
        };
        if removal.frames.total > 0 {
            anyhow::bail!(
                "project {} still has {} lights; nothing was changed",
                project.name,
                removal.frames.total
            );
        }
        project_removal::remove_planned(&tx, &removal, now)?;
    }
    crate::db::record_rejection_times(&tx)?;
    tx.commit()?;
    Ok(FoldOutcome::Folded(plan))
}

/// What a fold on a database file did, and the copy taken first; no copy
/// when there was nothing to do or the catalog changed.
#[derive(Debug)]
pub struct RepairOutcome {
    pub outcome: FoldOutcome,
    pub backup: Option<PathBuf>,
}

/// The whole repair on a database file: refuse a file this process cannot
/// write, copy it beside itself when there is something to do, then fold.
/// Runs on its own connection.
pub fn repair_database(
    path: &std::path::Path,
    digest: Option<&str>,
    keepers: &Keepers,
) -> Result<RepairOutcome> {
    let mut conn = crate::ts_guids::open_existing(path, true)?;
    if conn.is_readonly(rusqlite::MAIN_DB)? {
        anyhow::bail!(
            "{} is read only here; nothing was copied or changed",
            path.display()
        );
    }
    let preview = plan(&conn, keepers)?;
    if digest.is_some_and(|digest| digest != preview.digest) {
        return Ok(RepairOutcome {
            outcome: FoldOutcome::Changed(preview),
            backup: None,
        });
    }
    if preview.is_empty() {
        return Ok(RepairOutcome {
            outcome: FoldOutcome::Folded(preview),
            backup: None,
        });
    }
    let backup = crate::ts_guids::backup_beside_as(path, "imported-copies")?;
    let outcome = fold(&mut conn, Some(&preview.digest), keepers)?;
    Ok(RepairOutcome {
        outcome,
        backup: Some(backup),
    })
}

#[cfg(test)]
mod tests;
