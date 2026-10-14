//! How far each rig has got with each part of a plan: the frames its Target
//! Scheduler exposure plans ask for, have taken, accepted and rejected, per
//! objective, and the state of its project. Read only.
//!
//! An exposure plan belongs to the objectives its activation wrote it for:
//! one, or several whose frames it sums, when it counts toward each and once
//! in the rig's total. One no activation recorded (a plan taken in from
//! Target Scheduler, or rows made by hand) goes to the rig's objective with
//! its template and exposure length, else the one with its template, else
//! the one with its filter's band; the rest are counted apart.

use super::*;
use psf_guard_director_core::bandpass::bandpass_for_filter;
use psf_guard_director_meta::plan::Contribution;
use rusqlite::{OpenFlags, OptionalExtension};
use std::{collections::BTreeSet, time::Duration};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(super) struct Frames {
    desired: i64,
    acquired: i64,
    accepted: i64,
    /// Graded rejected; acquired minus accepted minus rejected is pending.
    rejected: i64,
}

impl Frames {
    fn add(&mut self, other: Frames) {
        self.desired += other.desired;
        self.acquired += other.acquired;
        self.accepted += other.accepted;
        self.rejected += other.rejected;
    }
}

#[derive(Debug, Serialize)]
pub(super) struct ObjectiveProgress {
    objective_id: Uuid,
    frames: Frames,
    /// How many exposure plans count toward it; 0 until one is written.
    exposure_plans: usize,
    /// Objectives that share an exposure plan with it: the plan sums their
    /// frames, and its counts show under each.
    shared_with: Vec<Uuid>,
}

#[derive(Debug, Serialize)]
pub(super) struct ProjectState {
    name: String,
    /// Target Scheduler's state: 0 draft, 1 active, 2 inactive, 3 closed.
    state: i64,
}

#[derive(Debug, Serialize)]
pub(super) struct RigProgress {
    rig_id: Uuid,
    /// The rig's Target Scheduler project for this plan, once it has one.
    project: Option<ProjectState>,
    objectives: Vec<ObjectiveProgress>,
    /// Exposure plans in the project that no objective of this rig claims.
    other: Frames,
    total: Frames,
    /// Why the rig's progress could not be read.
    note: Option<String>,
}

#[derive(Serialize)]
pub(super) struct Progress {
    rigs: Vec<RigProgress>,
}

/// One rig's part of the plan, and where to read its frames.
struct RigRead {
    rig: Uuid,
    path: Option<String>,
    project_guid: Option<Uuid>,
    contributions: Vec<Contribution>,
    /// Objective bands, by objective.
    bands: BTreeMap<Uuid, String>,
    /// Exposure plan GUID to the objectives it serves, from the last
    /// activation.
    activated: BTreeMap<Uuid, Vec<Uuid>>,
}

pub(super) async fn progress(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Progress>>, Error> {
    let service = enabled(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let instance = service.instance_id;
    let reads = service
        .with_reader(move |store| -> Result<Vec<RigRead>, Error> {
            store.project(id)?.ok_or(Error::Missing)?;
            let Some(plan) = store.plan_draft(id)? else {
                return Ok(vec![]);
            };
            let activation = store.activation(id)?;
            let bands: BTreeMap<Uuid, String> = plan
                .objectives
                .iter()
                .map(|o| (o.id, o.bandpass_id.clone()))
                .collect();
            let rigs: BTreeSet<Uuid> = plan.contributions.iter().map(|c| c.rig_id).collect();
            let found = identified_catalogs(&catalogs, instance);
            let mut reads = Vec::new();
            for rig in rigs {
                let mut path = None;
                let mut project_guid = None;
                for (identity, context) in found.iter() {
                    if !store
                        .catalog_rig(identity.id)?
                        .is_some_and(|binding| binding.rig.id == rig)
                    {
                        continue;
                    }
                    path = Some(context.database_path.clone());
                    let mut after = None;
                    while project_guid.is_none() {
                        let page = store.catalog_project_mappings(identity.id, after, 256)?;
                        project_guid = page
                            .items
                            .iter()
                            .find(|m| m.project_id == id)
                            .map(|m| m.source_project_guid);
                        match page.next_after {
                            Some(next) => after = Some(next),
                            None => break,
                        }
                    }
                    break;
                }
                let activated_rig = activation
                    .as_ref()
                    .and_then(|a| a.rigs.iter().find(|r| r.rig_id == rig));
                project_guid = project_guid.or(activated_rig.map(|r| r.project_guid));
                let mut activated: BTreeMap<Uuid, Vec<Uuid>> = BTreeMap::new();
                for plan in activated_rig.iter().flat_map(|r| &r.plans) {
                    let objectives = activated.entry(plan.exposureplan_guid).or_default();
                    if !objectives.contains(&plan.objective_id) {
                        objectives.push(plan.objective_id);
                    }
                }
                reads.push(RigRead {
                    rig,
                    path,
                    project_guid,
                    contributions: plan
                        .contributions
                        .iter()
                        .filter(|c| c.rig_id == rig)
                        .cloned()
                        .collect(),
                    bands: bands.clone(),
                    activated,
                });
            }
            Ok(reads)
        })
        .await?;
    // Rig databases are read with nothing of the store held.
    let rigs = tokio::task::spawn_blocking(move || reads.into_iter().map(read_rig).collect())
        .await
        .map_err(|_| Error::Internal)?;
    Ok(Json(ApiResponse::success(Progress { rigs })))
}

fn read_rig(read: RigRead) -> RigProgress {
    let mut progress = RigProgress {
        rig_id: read.rig,
        project: None,
        objectives: vec![],
        other: Frames::default(),
        total: Frames::default(),
        note: None,
    };
    let (Some(path), Some(guid)) = (read.path.as_deref(), read.project_guid) else {
        if read.path.is_none() {
            progress.note = Some("This rig has no registered database on this server.".into());
        }
        progress.objectives = objectives_of(&read, &BTreeMap::new());
        return progress;
    };
    match frames_of(path, guid, &read) {
        Ok(counted) => {
            progress.project = counted.project;
            progress.objectives = objectives_of(&read, &counted.objectives);
            progress.other = counted.other;
            progress.total = counted.total;
        }
        Err(error) => {
            progress.note = Some(format!("Its database could not be read: {error}."));
            progress.objectives = objectives_of(&read, &BTreeMap::new());
        }
    }
    progress
}

/// One entry per objective the rig has a contribution for, in plan order.
fn objectives_of(read: &RigRead, counted: &BTreeMap<Uuid, Counts>) -> Vec<ObjectiveProgress> {
    let mut seen = BTreeSet::new();
    read.contributions
        .iter()
        .filter(|c| seen.insert(c.objective_id))
        .map(|c| {
            let counts = counted.get(&c.objective_id);
            ObjectiveProgress {
                objective_id: c.objective_id,
                frames: counts.map(|c| c.frames).unwrap_or_default(),
                exposure_plans: counts.map_or(0, |c| c.plans),
                shared_with: counts
                    .map(|c| c.shared_with.iter().copied().collect())
                    .unwrap_or_default(),
            }
        })
        .collect()
}

/// What counts toward one objective.
#[derive(Default)]
struct Counts {
    frames: Frames,
    plans: usize,
    shared_with: BTreeSet<Uuid>,
}

/// A rig's project as read: its state, each objective's counts, the plans
/// no objective claims, and every plan an objective claims counted once.
#[derive(Default)]
struct Counted {
    project: Option<ProjectState>,
    objectives: BTreeMap<Uuid, Counts>,
    other: Frames,
    total: Frames,
}

fn frames_of(path: &str, project: Uuid, read: &RigRead) -> rusqlite::Result<Counted> {
    let connection = crate::server::database_context::open_scheduler_connection_with_flags(
        std::path::Path::new(path),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(2))?;
    let row: Option<(i64, String, i64)> = connection
        .query_row(
            "SELECT Id, IFNULL(name, ''), IFNULL(state, 0) FROM project WHERE lower(guid)=lower(?1)",
            [project.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((row_id, name, state)) = row else {
        return Ok(Counted::default());
    };
    let plan_guid = if import_drafts::has_column(&connection, "exposureplan", "guid") {
        "e.guid"
    } else {
        "NULL"
    };
    let template_guid = if import_drafts::has_column(&connection, "exposuretemplate", "guid") {
        "x.guid"
    } else {
        "NULL"
    };
    // Rejected frames name their exposure plan from Target Scheduler 5 on.
    let mut rejected: BTreeMap<i64, i64> = BTreeMap::new();
    if import_drafts::has_column(&connection, "acquiredimage", "exposureId")
        && import_drafts::has_column(&connection, "acquiredimage", "gradingStatus")
    {
        let mut statement = connection.prepare(
            "SELECT exposureId, COUNT(*) FROM acquiredimage
             WHERE projectId=?1 AND gradingStatus=2 GROUP BY exposureId",
        )?;
        let rows = statement.query_map([row_id], |row| Ok((row.get(0)?, row.get(1)?)))?;
        for row in rows {
            let (plan, count): (Option<i64>, i64) = row?;
            if let Some(plan) = plan {
                rejected.insert(plan, count);
            }
        }
    }
    // Rejects removed from the catalog still count as rejected.
    if import_drafts::has_column(&connection, "psf_guard_removed_image", "exposure_id") {
        let mut statement = connection.prepare(
            "SELECT exposure_id, COUNT(*) FROM psf_guard_removed_image
             WHERE project_id = ?1 AND exposure_id IS NOT NULL GROUP BY exposure_id",
        )?;
        let rows = statement.query_map([row_id], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in rows {
            let (plan, count) = row?;
            *rejected.entry(plan).or_default() += count;
        }
    }
    let mut statement = connection.prepare(&format!(
        "SELECT e.Id, {plan_guid}, IFNULL(e.desired, 0), IFNULL(e.acquired, 0), IFNULL(e.accepted, 0),
                CASE WHEN e.exposure IS NULL OR e.exposure <= 0 THEN IFNULL(x.defaultexposure, 0) ELSE e.exposure END,
                e.exposureTemplateId, {template_guid}, IFNULL(x.filtername, '')
         FROM exposureplan e JOIN target t ON t.Id = e.targetid
         LEFT JOIN exposuretemplate x ON x.Id = e.exposureTemplateId
         WHERE t.projectid = ?1 ORDER BY e.Id"
    ))?;
    let rows = statement.query_map([row_id], |row| {
        Ok(Row {
            id: row.get(0)?,
            guid: row.get::<_, Option<String>>(1)?,
            frames: Frames {
                desired: row.get(2)?,
                acquired: row.get(3)?,
                accepted: row.get(4)?,
                rejected: 0,
            },
            exposure: row.get(5)?,
            template_id: row.get(6)?,
            template_guid: row.get::<_, Option<String>>(7)?,
            filter: row.get(8)?,
        })
    })?;
    let mut counted = Counted {
        project: Some(ProjectState { name, state }),
        ..Counted::default()
    };
    for row in rows {
        let mut row = row?;
        row.frames.rejected = rejected.get(&row.id).copied().unwrap_or(0);
        let objectives = objectives_for(&row, read);
        if objectives.is_empty() {
            counted.other.add(row.frames);
            continue;
        }
        counted.total.add(row.frames);
        for objective in &objectives {
            let entry = counted.objectives.entry(*objective).or_default();
            entry.frames.add(row.frames);
            entry.plans += 1;
            entry
                .shared_with
                .extend(objectives.iter().filter(|other| *other != objective));
        }
    }
    Ok(counted)
}

struct Row {
    id: i64,
    guid: Option<String>,
    frames: Frames,
    exposure: f64,
    template_id: Option<i64>,
    template_guid: Option<String>,
    filter: String,
}

/// The objectives an exposure plan counts toward: the ones its activation
/// wrote it for, else the rig's with its template and exposure length (the
/// row's own, or its template's default), else the rig's with its
/// template, else the only one with its band.
fn objectives_for(row: &Row, read: &RigRead) -> Vec<Uuid> {
    let parse = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|v| Uuid::parse_str(v.trim()).ok())
    };
    if let Some(objectives) = parse(&row.guid).and_then(|guid| read.activated.get(&guid)) {
        return objectives.clone();
    }
    objective_for(row, read).into_iter().collect()
}

fn objective_for(row: &Row, read: &RigRead) -> Option<Uuid> {
    let parse = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|v| Uuid::parse_str(v.trim()).ok())
    };
    let template_guid = parse(&row.template_guid);
    let by_template: Vec<&Contribution> = read
        .contributions
        .iter()
        .filter(|c| {
            (row.template_id.is_some() && c.template.template_id == row.template_id)
                || (template_guid.is_some() && c.template.template_guid == template_guid)
        })
        .collect();
    let unique = |found: Vec<&Contribution>| -> Option<Uuid> {
        let objectives: BTreeSet<Uuid> = found.iter().map(|c| c.objective_id).collect();
        (objectives.len() == 1).then(|| *objectives.first().unwrap())
    };
    if let Some(objective) = unique(
        by_template
            .iter()
            .copied()
            .filter(|c| (c.exposure_seconds - row.exposure).abs() < 0.5)
            .collect(),
    ) {
        return Some(objective);
    }
    if let Some(objective) = unique(by_template.clone()) {
        return Some(objective);
    }
    if by_template.len() > 1 {
        return None;
    }
    let band = bandpass_for_filter(&row.filter).id;
    let by_band: BTreeSet<Uuid> = read
        .contributions
        .iter()
        .filter(|c| read.bands.get(&c.objective_id) == Some(&band))
        .map(|c| c.objective_id)
        .collect();
    (by_band.len() == 1).then(|| *by_band.first().unwrap())
}
