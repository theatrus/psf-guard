//! The Director page's plan list: every global project with where it is
//! linked, and how far its framing, plan and activation have come. Read only.

use super::rig_profile;
use super::*;
use crate::catalog_identity;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::{
    framing::{FramingRequest, Mosaic, PanelSize},
    visibility::IcrsPosition,
};
use psf_guard_director_meta::{catalog::ProjectMapping, profile::RigProfile};
use rusqlite::{OpenFlags, TransactionBehavior};
use std::{collections::BTreeMap, time::Duration};

/// One target of a linked project and its frames: what the exposure plans
/// ask for, have, and have accepted. A mosaic has one per panel.
#[derive(Clone, Serialize)]
struct TargetProgress {
    name: String,
    desired: i64,
    acquired: i64,
    accepted: i64,
    /// Frames graded rejected in `acquiredimage`; the rest of `acquired`
    /// minus `accepted` is still pending.
    rejected: i64,
    /// Where Target Scheduler points this target, in ICRS degrees, when the
    /// row has coordinates.
    center: Option<IcrsPosition>,
    rotation_degrees: Option<f64>,
}

#[derive(Serialize)]
struct PlanLink {
    catalog_slug: String,
    catalog_name: String,
    rig: NamedIdentity,
    source_project_guid: Uuid,
    /// The row and name in that database, when the project still exists there.
    source_row_id: Option<i64>,
    source_name: Option<String>,
    /// Target Scheduler's project state there: 0 draft, 1 active, 2
    /// inactive, 3 closed; None when the row is gone.
    source_state: Option<i32>,
    /// First and last capture of the project's frames there, Unix seconds.
    earliest_capture_s: Option<i64>,
    latest_capture_s: Option<i64>,
    /// The project's targets in that database, in row order.
    targets: Vec<TargetProgress>,
}

/// What the Library shows for a project and the plan list shows per rig:
/// its Target Scheduler state and when its frames were taken.
#[derive(Clone, Copy, Default)]
struct ProjectFacts {
    state: Option<i32>,
    earliest_capture_s: Option<i64>,
    latest_capture_s: Option<i64>,
}

/// Per project row, its state and capture dates; a schema without the
/// columns reports none rather than failing the list.
fn project_facts(connection: &rusqlite::Connection) -> BTreeMap<i64, ProjectFacts> {
    let mut facts: BTreeMap<i64, ProjectFacts> = BTreeMap::new();
    if let Ok(mut statement) = connection.prepare("SELECT Id, state FROM project")
        && let Ok(rows) = statement.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Option<i32>>(1)?))
        })
    {
        for (id, state) in rows.flatten() {
            facts.entry(id).or_default().state = state;
        }
    }
    if let Ok(mut statement) = connection.prepare(
        "SELECT t.projectid, MIN(a.acquireddate), MAX(a.acquireddate)
         FROM acquiredimage a JOIN target t ON a.targetId = t.Id
         WHERE a.acquireddate IS NOT NULL GROUP BY t.projectid",
    ) && let Ok(rows) = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, Option<i64>>(1)?,
            row.get::<_, Option<i64>>(2)?,
        ))
    }) {
        for (id, earliest, latest) in rows.flatten() {
            let entry = facts.entry(id).or_default();
            entry.earliest_capture_s = earliest;
            entry.latest_capture_s = latest;
        }
    }
    facts
}

/// Frames across every linked database, so the list can say how far a plan
/// has come without opening it.
#[derive(Serialize)]
struct Progress {
    desired: i64,
    acquired: i64,
    accepted: i64,
    rejected: i64,
    targets: u32,
}

/// Enough of the framing for the plan list to draw a thumbnail: the
/// survey view at the center, with the panel rectangles over it.
#[derive(Serialize)]
struct FramingSummary {
    /// `draft`: a framing saved in Director. `catalog`: no draft yet, so the
    /// first linked database's target stands in, with the rig's field as
    /// the panel when its optics are known.
    source: &'static str,
    revision: u64,
    target_name: String,
    panels: u32,
    panel_rig_id: Option<Uuid>,
    center: IcrsPosition,
    position_angle_degrees: f64,
    panel: Option<PanelSize>,
    mosaic: Mosaic,
    survey_id: String,
    /// The whole mosaic along the camera axes, once a panel size is known.
    extent: Option<PanelSize>,
}

#[derive(Serialize)]
struct PlanSummary {
    revision: u64,
    objectives: u32,
    rigs: u32,
}

#[derive(Serialize)]
struct ActivationSummary {
    revision: u64,
    applied_at_ms: u64,
    rigs: u32,
}

#[derive(Serialize)]
pub(super) struct PlanRow {
    project: NamedIdentity,
    links: Vec<PlanLink>,
    /// `None` until some linked database holds a target for the project.
    progress: Option<Progress>,
    framing: Option<FramingSummary>,
    plan: Option<PlanSummary>,
    activation: Option<ActivationSummary>,
}

const MAX_PROJECTS: usize = 1024;

/// Every target's frame counts, grouped by project row. A catalog whose
/// schema lacks the columns reports no progress rather than failing the list.
fn target_progress(connection: &rusqlite::Connection) -> BTreeMap<i64, Vec<TargetProgress>> {
    let mut by_project: BTreeMap<i64, Vec<TargetProgress>> = BTreeMap::new();
    // `rotation` arrived with a later Target Scheduler schema.
    let rotation = if connection
        .prepare("SELECT rotation FROM target LIMIT 0")
        .is_ok()
    {
        "t.rotation"
    } else {
        "NULL"
    };
    // Rejected frames live in `acquiredimage`; a pre-TS5 file names the column
    // `accepted` and then reports none.
    let rejected: BTreeMap<i64, i64> = connection
        .prepare("SELECT targetId, COUNT(*) FROM acquiredimage WHERE gradingStatus = 2 GROUP BY targetId")
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))
                .map(|rows| rows.flatten().collect())
        })
        .unwrap_or_default();
    let Ok(mut statement) = connection.prepare(&format!(
        "SELECT t.projectid, t.name, COALESCE(SUM(e.desired), 0), COALESCE(SUM(e.acquired), 0), COALESCE(SUM(e.accepted), 0),
                t.ra, t.dec, {rotation}, t.Id
         FROM target t LEFT JOIN exposureplan e ON e.targetid = t.Id
         GROUP BY t.Id ORDER BY t.Id"
    )) else {
        return by_project;
    };
    let rows = statement.query_map([], |row| {
        let ra_hours: Option<f64> = row.get(5)?;
        let dec: Option<f64> = row.get(6)?;
        let center = match (ra_hours, dec) {
            (Some(ra_hours), Some(dec_degrees)) => {
                crate::astrometry::target_scheduler_coordinates(ra_hours, dec_degrees).map(
                    |(ra_degrees, dec_degrees)| IcrsPosition {
                        ra_degrees,
                        dec_degrees,
                    },
                )
            }
            _ => None,
        };
        Ok((
            row.get::<_, i64>(0)?,
            TargetProgress {
                name: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                desired: row.get(2)?,
                acquired: row.get(3)?,
                accepted: row.get(4)?,
                rejected: rejected.get(&row.get::<_, i64>(8)?).copied().unwrap_or(0),
                center,
                rotation_degrees: row.get::<_, Option<f64>>(7)?.filter(|r| r.is_finite()),
            },
        ))
    });
    if let Ok(rows) = rows {
        for (project, target) in rows.flatten() {
            by_project.entry(project).or_default().push(target);
        }
    }
    by_project
}

#[derive(Serialize)]
pub(super) struct PlanList {
    rows: Vec<PlanRow>,
    /// Databases the automatic adoption could not take in, and why.
    warnings: Vec<String>,
}

/// Every registered database is a rig and every Target Scheduler project in
/// it is a plan, without an operator step. Projects that share a GUID across
/// databases, as Sync copies do, become one plan with several rigs; projects
/// that merely share a name stay apart. A database that cannot be written
/// or has no usable GUIDs is reported, not failed.
///
/// Without database management the file is only read: it is planned under
/// its derived identity, and the identity table is written the first time a
/// managing server lists it, under the same id.
fn ensure_adopted(
    store: &mut MetaStore,
    instance: Uuid,
    catalog: &DatabaseContext,
    management: bool,
) -> Result<(), String> {
    let flags = if management {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(&catalog.database_path),
        flags | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| format!("{}: could not be opened for planning", catalog.name))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| format!("{}: busy", catalog.name))?;
    let mut tx = connection
        .transaction_with_behavior(if management {
            TransactionBehavior::Immediate
        } else {
            TransactionBehavior::Deferred
        })
        .map_err(|_| format!("{}: busy", catalog.name))?;
    let evidence = match catalog_discovery::read_evidence(&tx) {
        Ok(evidence) => evidence,
        Err(_) => {
            return Err(format!(
                "{}: has no Target Scheduler project table",
                catalog.name
            ))
        }
    };
    let saved = catalog_identity::read(&tx)
        .map_err(|_| format!("{}: identity unreadable", catalog.name))?;
    let unwritten = saved.is_none();
    let identity =
        saved.unwrap_or_else(|| super::derived_identity(instance, &catalog.database_path));
    let binding = match store.catalog_rig(identity.id) {
        Ok(Some(binding)) => {
            // Bound while read-only; the file takes its identity now.
            if unwritten && management {
                catalog_identity::adopt(&mut tx, identity)
                    .map_err(|_| format!("{}: identity could not be written", catalog.name))?;
            }
            binding
        }
        Ok(None) => store
            .bind_catalog_rig_after(identity, &catalog.name, unwritten, || {
                if management {
                    catalog_identity::adopt(&mut tx, identity).map_err(|_| StoreError::Conflict)?;
                }
                Ok(())
            })
            .map_err(|error| format!("{}: could not be bound to a rig ({error})", catalog.name))?,
        Err(error) => return Err(format!("{}: {error}", catalog.name)),
    };
    // A rig with no optics yet takes them from its own frames, so the framing
    // view can draw its rectangle at once. The source says where they came from.
    match store.rig_profile(binding.rig.id) {
        Ok(profile) if profile.as_ref().is_none_or(|p| p.optics.is_none()) => {
            let defaults = rig_profile::header_defaults(catalog, &tx);
            if defaults.optics.is_some() || defaults.site.is_some() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let revision = profile.as_ref().map_or(0, |p| p.revision);
                let mut next = profile.unwrap_or_else(|| RigProfile::empty(binding.rig.id, now));
                if next.optics.is_none() {
                    next.optics = defaults.optics;
                }
                if next.site.is_none() {
                    next.site = defaults.site;
                }
                next.updated_at_ms = now;
                if let Err(error) = store.save_rig_profile(&next, revision) {
                    tracing::warn!(?error, catalog = %catalog.name, "Header optics were not saved to the rig profile");
                }
            }
        }
        Ok(_) => {}
        Err(error) => return Err(format!("{}: {error}", catalog.name)),
    }
    // Which source projects still need a plan.
    let mut mappings = Vec::new();
    for (guid, _row, name) in evidence.identified_rows() {
        if store
            .linked_project(identity.id, guid)
            .map_err(|error| format!("{}: {error}", catalog.name))?
            .is_some()
        {
            continue;
        }
        let profile = evidence
            .profile_of(guid)
            .ok_or_else(|| format!("{}: project {guid} has no profile", catalog.name))?;
        let project_id = match store
            .project_for_source_guid(guid)
            .map_err(|error| format!("{}: {error}", catalog.name))?
        {
            Some(existing) => existing,
            None => {
                let label = name.unwrap_or("Project").trim();
                let label = if label.is_empty() { "Project" } else { label };
                store
                    .create_project(Uuid::new_v4(), label)
                    .map_err(|error| {
                        format!(
                            "{}: could not create plan for {label} ({error})",
                            catalog.name
                        )
                    })?
                    .id
            }
        };
        mappings.push(ProjectMapping {
            catalog_id: identity.id,
            source_project_guid: guid,
            source_profile_id: profile,
            project_id,
            rig_id: binding.rig.id,
        });
    }
    if !mappings.is_empty() {
        for chunk in mappings.chunks(256) {
            store.link_catalog_projects(chunk).map_err(|error| {
                format!("{}: could not link its projects ({error})", catalog.name)
            })?;
        }
    }
    tx.commit()
        .map_err(|_| format!("{}: identity could not be saved", catalog.name))?;
    Ok(())
}

/// A framing summary from the first linked target with coordinates: its
/// center and rotation, the rig's field as the panel when the rig profile
/// holds optics, and every target of that database counted as a panel.
fn catalog_framing(
    store: &MetaStore,
    links: &[PlanLink],
) -> Result<Option<FramingSummary>, StoreError> {
    for link in links {
        let Some(target) = link.targets.iter().find(|t| t.center.is_some()) else {
            continue;
        };
        let center = target.center.expect("filtered on Some");
        let panel = store
            .rig_profile(link.rig.id)?
            .and_then(|profile| profile.optics)
            .and_then(|optics| optics.value.field_of_view().ok())
            .map(|fov| PanelSize {
                width_degrees: fov.width_degrees,
                height_degrees: fov.height_degrees,
            });
        return Ok(Some(FramingSummary {
            source: "catalog",
            revision: 0,
            target_name: target.name.clone(),
            panels: link.targets.len() as u32,
            panel_rig_id: panel.is_some().then_some(link.rig.id),
            center,
            position_angle_degrees: target.rotation_degrees.unwrap_or(0.0).rem_euclid(360.0),
            panel,
            mosaic: Mosaic {
                rows: 1,
                columns: 1,
                overlap_percent: 20,
            },
            survey_id: "dss2_color".to_owned(),
            extent: panel,
        }));
    }
    Ok(None)
}

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<PlanList>>, Error> {
    let service = enabled(&state)?;
    let management = state.database_management_allowed();
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let catalog_permit = admit(&service.discovery_admission).await?;
    let list = service
        .clone()
        .with_writer(move |store| {
            let _catalog_permit = catalog_permit;
            // A hand-copied file carries its original's identity: name it and
            // leave it alone, so it never gets plans or links of its own.
            let before = identified_catalogs(&catalogs, service.instance_id);
            let mut warnings = before.duplicates.clone();
            for catalog in &catalogs {
                if before.is_duplicate(&catalog.id) {
                    continue;
                }
                if let Err(warning) =
                    ensure_adopted(store, service.instance_id, catalog, management)
                {
                    warnings.push(warning);
                }
            }
            // Links first: each bound database's mappings, joined to its rows.
            let mut links: BTreeMap<Uuid, Vec<PlanLink>> = BTreeMap::new();
            for (identity, catalog) in identified_catalogs(&catalogs, service.instance_id).iter() {
                let Ok(connection) =
                    super::super::database_context::open_scheduler_connection_with_flags(
                        FilePath::new(&catalog.database_path),
                        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    )
                else {
                    continue;
                };
                if connection.busy_timeout(Duration::from_secs(1)).is_err() {
                    continue;
                }
                let Some(binding) = store.catalog_rig(identity.id)? else {
                    continue;
                };
                let rows: BTreeMap<Uuid, (i64, Option<String>)> =
                    match catalog_discovery::read_evidence(&connection) {
                        Ok(evidence) => evidence
                            .identified_rows()
                            .map(|(guid, row, name)| (guid, (row, name.map(str::to_owned))))
                            .collect(),
                        Err(_) => BTreeMap::new(),
                    };
                let progress = target_progress(&connection);
                let facts = project_facts(&connection);
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                let mut after = None;
                loop {
                    let page = store.catalog_project_mappings(identity.id, after, 256)?;
                    for mapping in &page.items {
                        let row = rows.get(&mapping.source_project_guid);
                        // What Target Scheduler already holds for the project is
                        // its plan: take it in as drafts the first time, once.
                        if let Some((row_id, name)) = row {
                            let label = name.as_deref().unwrap_or("Project");
                            match super::import_drafts::import_from_catalog(
                                &mut *store,
                                &connection,
                                binding.rig.id,
                                mapping.project_id,
                                *row_id,
                                label,
                                now,
                            ) {
                                Ok(imported) if imported.separate_targets > 1 => {
                                    warnings.push(format!(
                                        "{}: {label} has {} separate targets; its plan frames the first. Target Scheduler keeps running the others.",
                                        catalog.name, imported.separate_targets
                                    ));
                                }
                                Ok(_) => {}
                                Err(error) => warnings.push(format!(
                                    "{}: {label} could not be imported into Director ({error})",
                                    catalog.name
                                )),
                            }
                        }
                        links.entry(mapping.project_id).or_default().push(PlanLink {
                            catalog_slug: catalog.id.clone(),
                            catalog_name: catalog.name.clone(),
                            rig: binding.rig.clone(),
                            source_project_guid: mapping.source_project_guid,
                            source_row_id: row.map(|(id, _)| *id),
                            source_name: row.and_then(|(_, name)| name.clone()),
                            source_state: row
                                .and_then(|(id, _)| facts.get(id))
                                .and_then(|f| f.state),
                            earliest_capture_s: row
                                .and_then(|(id, _)| facts.get(id))
                                .and_then(|f| f.earliest_capture_s),
                            latest_capture_s: row
                                .and_then(|(id, _)| facts.get(id))
                                .and_then(|f| f.latest_capture_s),
                            targets: row
                                .and_then(|(id, _)| progress.get(id))
                                .cloned()
                                .unwrap_or_default(),
                        });
                    }
                    match page.next_after {
                        Some(next) if links.values().map(Vec::len).sum::<usize>() < 4096 => {
                            after = Some(next)
                        }
                        _ => break,
                    }
                }
            }
            let mut projects = Vec::new();
            let mut after = None;
            loop {
                let page = store.projects(after, 256)?;
                projects.extend(page.items);
                match page.next_after {
                    Some(next) if projects.len() < MAX_PROJECTS => after = Some(next),
                    _ => break,
                }
            }
            // Databases come from a map; name order keeps the first link stable.
            for entries in links.values_mut() {
                entries.sort_by(|a, b| {
                    a.catalog_name
                        .cmp(&b.catalog_name)
                        .then_with(|| a.source_row_id.cmp(&b.source_row_id))
                });
            }
            let mut rows = Vec::with_capacity(projects.len());
            for project in projects {
                let framing = store.framing_draft(project.id)?.map(|draft| {
                    let preview = draft.panel.and_then(|panel| {
                        FramingRequest {
                            center: draft.center,
                            position_angle_degrees: draft.position_angle_degrees,
                            panel,
                            mosaic: draft.mosaic,
                            overlays: vec![],
                            view: None,
                        }
                        .preview()
                        .ok()
                    });
                    FramingSummary {
                        source: "draft",
                        revision: draft.revision,
                        target_name: draft.target_name.clone(),
                        panels: preview
                            .as_ref()
                            .map(|p| p.panels.len() as u32)
                            // No panel size yet; the grid still says how many.
                            .unwrap_or(draft.mosaic.rows * draft.mosaic.columns),
                        panel_rig_id: draft.panel_rig_id,
                        center: draft.center,
                        position_angle_degrees: draft.position_angle_degrees,
                        panel: draft.panel,
                        mosaic: draft.mosaic,
                        survey_id: draft.survey_id.clone(),
                        extent: preview.map(|p| p.extent),
                    }
                });
                let plan = store.plan_draft(project.id)?.map(|plan| PlanSummary {
                    revision: plan.revision,
                    objectives: plan.objectives.len() as u32,
                    rigs: plan
                        .contributions
                        .iter()
                        .filter(|c| c.enabled)
                        .map(|c| c.rig_id)
                        .collect::<std::collections::BTreeSet<_>>()
                        .len() as u32,
                });
                let activation = store.activation(project.id)?.map(|a| ActivationSummary {
                    revision: a.revision,
                    applied_at_ms: a.applied_at_ms,
                    rigs: a.rigs.len() as u32,
                });
                let links = links.remove(&project.id).unwrap_or_default();
                // No draft yet, but Target Scheduler already points somewhere:
                // that is the framing until Director saves one of its own.
                let framing = match framing {
                    Some(framing) => Some(framing),
                    None => catalog_framing(store, &links)?,
                };
                let targets: Vec<&TargetProgress> = links.iter().flat_map(|l| &l.targets).collect();
                let progress = (!targets.is_empty()).then(|| Progress {
                    desired: targets.iter().map(|t| t.desired).sum(),
                    acquired: targets.iter().map(|t| t.acquired).sum(),
                    accepted: targets.iter().map(|t| t.accepted).sum(),
                    rejected: targets.iter().map(|t| t.rejected).sum(),
                    targets: targets.len() as u32,
                });
                rows.push(PlanRow {
                    links,
                    progress,
                    project,
                    framing,
                    plan,
                    activation,
                });
            }
            Ok::<_, Error>(PlanList { rows, warnings })
        })
        .await?;
    Ok(Json(ApiResponse::success(list)))
}
