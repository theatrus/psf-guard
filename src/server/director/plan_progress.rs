//! How far each rig has got with each part of a plan: the frames its Target
//! Scheduler exposure plans ask for, have taken, accepted and rejected, per
//! objective, and the state of its project. Read only.
//!
//! An exposure plan belongs to the objective its activation wrote it for.
//! One no activation recorded (a plan taken in from Target Scheduler, or
//! rows made by hand) goes to the rig's objective with its template, else
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
    /// Exposure plan GUID to objective, from the last activation.
    activated: BTreeMap<Uuid, Uuid>,
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
                let activated = activated_rig
                    .map(|r| {
                        r.plans
                            .iter()
                            .map(|p| (p.exposureplan_guid, p.objective_id))
                            .collect()
                    })
                    .unwrap_or_default();
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
        Ok((project, by_objective, other)) => {
            progress.project = project;
            progress.objectives = objectives_of(&read, &by_objective);
            progress.other = other;
            for objective in &progress.objectives {
                progress.total.add(objective.frames);
            }
        }
        Err(error) => {
            progress.note = Some(format!("Its database could not be read: {error}."));
            progress.objectives = objectives_of(&read, &BTreeMap::new());
        }
    }
    progress
}

/// One entry per objective the rig has a contribution for, in plan order.
fn objectives_of(
    read: &RigRead,
    counted: &BTreeMap<Uuid, (Frames, usize)>,
) -> Vec<ObjectiveProgress> {
    let mut seen = BTreeSet::new();
    read.contributions
        .iter()
        .filter(|c| seen.insert(c.objective_id))
        .map(|c| {
            let (frames, plans) = counted.get(&c.objective_id).copied().unwrap_or_default();
            ObjectiveProgress {
                objective_id: c.objective_id,
                frames,
                exposure_plans: plans,
            }
        })
        .collect()
}

type Counted = (
    Option<ProjectState>,
    BTreeMap<Uuid, (Frames, usize)>,
    Frames,
);

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
        return Ok((None, BTreeMap::new(), Frames::default()));
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
    let mut statement = connection.prepare(&format!(
        "SELECT e.Id, {plan_guid}, IFNULL(e.desired, 0), IFNULL(e.acquired, 0), IFNULL(e.accepted, 0),
                IFNULL(e.exposure, 0), e.exposureTemplateId, {template_guid}, IFNULL(x.filtername, '')
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
    let mut counted: BTreeMap<Uuid, (Frames, usize)> = BTreeMap::new();
    let mut other = Frames::default();
    for row in rows {
        let mut row = row?;
        row.frames.rejected = rejected.get(&row.id).copied().unwrap_or(0);
        match objective_for(&row, read) {
            Some(objective) => {
                let entry = counted.entry(objective).or_default();
                entry.0.add(row.frames);
                entry.1 += 1;
            }
            None => other.add(row.frames),
        }
    }
    Ok((Some(ProjectState { name, state }), counted, other))
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

/// The objective an exposure plan counts toward: the one its activation
/// wrote it for, else the rig's with its template (the exposure settling a
/// template two objectives share), else the only one with its band.
fn objective_for(row: &Row, read: &RigRead) -> Option<Uuid> {
    let parse = |value: &Option<String>| {
        value
            .as_deref()
            .and_then(|v| Uuid::parse_str(v.trim()).ok())
    };
    if let Some(objective) = parse(&row.guid).and_then(|guid| read.activated.get(&guid)) {
        return Some(*objective);
    }
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
    if let Some(objective) = unique(by_template.clone()) {
        return Some(objective);
    }
    if by_template.len() > 1 {
        return unique(
            by_template
                .into_iter()
                .filter(|c| (c.exposure_seconds - row.exposure).abs() < 0.5)
                .collect(),
        );
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
