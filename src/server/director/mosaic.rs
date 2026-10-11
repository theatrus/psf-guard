//! The mosaic preview: each panel's finished stacks, placed by their plate
//! solves over the plan's geometry. A review aid for coverage and seams,
//! never a processed image; panels from different rigs stay apart. The
//! panels are the targets an activation wrote, or for a rig no activation
//! reached, the targets of its linked Target Scheduler project: a plan taken
//! in from Target Scheduler that already matches it is never activated.

use super::*;
use crate::server::{
    database_context::DatabaseContext,
    sky_coverage::{stacks_for, SkyPreview},
};
use rusqlite::{OpenFlags, OptionalExtension};
use std::{collections::BTreeMap, time::Duration};

#[derive(Serialize)]
struct PanelProgress {
    desired: i64,
    acquired: i64,
    accepted: i64,
}

#[derive(Serialize)]
struct MosaicPanel {
    panel_id: String,
    rig: NamedIdentity,
    catalog_slug: Option<String>,
    catalog_name: String,
    /// The target's GUID; a row made by hand in an old schema may have none.
    target_guid: Option<Uuid>,
    /// The target's row and name in the rig database, when it still exists.
    target_id: Option<i64>,
    target_name: Option<String>,
    /// Frames the panel's exposure plans ask for, have, and have accepted.
    progress: Option<PanelProgress>,
    /// `ready`: a stack preview placed by its plate solve. `unsolved`: a stack
    /// exists but no solve reaches its grid, so it cannot be placed.
    /// `no_stack`, `missing_target`, `missing_catalog`: nothing to draw yet.
    status: &'static str,
    /// The stack drawn by default: the first of `stacks`.
    preview: Option<SkyPreview>,
    /// Every finished stack of the panel's target, best first.
    stacks: Vec<SkyPreview>,
}

#[derive(Serialize)]
pub(super) struct Mosaic {
    project: NamedIdentity,
    /// `None` until the project has been activated; panels then come from
    /// the linked Target Scheduler projects alone.
    activation_revision: Option<u64>,
    framing_revision: Option<u64>,
    /// The saved framing moved on since the activation. Solved stacks still
    /// sit where their solves put them; the plan's panels may have moved.
    framing_stale: bool,
    panels: Vec<MosaicPanel>,
    warnings: Vec<String>,
}

fn target_row(
    connection: &rusqlite::Connection,
    guid: Uuid,
) -> rusqlite::Result<Option<(i64, String)>> {
    connection
        .query_row(
            "SELECT Id, name FROM target WHERE guid = ?1",
            [guid.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

/// A panel's progress over the plans the activation gave it that are still
/// on: not the target's other plans, ones turned off, or twins.
fn progress(
    connection: &rusqlite::Connection,
    target_id: i64,
    plans: &[String],
) -> rusqlite::Result<PanelProgress> {
    let mut total = PanelProgress {
        desired: 0,
        acquired: 0,
        accepted: 0,
    };
    for guid in plans {
        let row: Option<(i64, i64, i64)> = connection
            .query_row(
                "SELECT IFNULL(desired, 0), IFNULL(acquired, 0), IFNULL(accepted, 0)
                 FROM exposureplan WHERE guid = ?1 AND targetid = ?2 AND COALESCE(enabled, 1) = 1",
                rusqlite::params![guid, target_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()?;
        if let Some((desired, acquired, accepted)) = row {
            total.desired += desired;
            total.acquired += acquired;
            total.accepted += accepted;
        }
    }
    Ok(total)
}

/// A linked project's targets: row, name and GUID.
fn project_targets(
    connection: &rusqlite::Connection,
    project: Uuid,
) -> rusqlite::Result<Vec<(i64, String, Option<Uuid>)>> {
    let mut statement = connection.prepare(
        "SELECT t.Id, IFNULL(t.name, ''), t.guid FROM target t JOIN project p ON p.Id = t.projectid
         WHERE lower(p.guid) = lower(?1) ORDER BY t.Id",
    )?;
    let rows = statement.query_map([project.to_string()], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?
                .and_then(|guid| Uuid::parse_str(guid.trim()).ok()),
        ))
    })?;
    rows.collect()
}

/// A target's progress over every exposure plan of it that is on.
fn target_progress(
    connection: &rusqlite::Connection,
    target_id: i64,
) -> rusqlite::Result<PanelProgress> {
    connection.query_row(
        "SELECT IFNULL(SUM(desired), 0), IFNULL(SUM(acquired), 0), IFNULL(SUM(accepted), 0)
         FROM exposureplan WHERE targetid = ?1 AND COALESCE(enabled, 1) = 1",
        [target_id],
        |row| {
            Ok(PanelProgress {
                desired: row.get(0)?,
                acquired: row.get(1)?,
                accepted: row.get(2)?,
            })
        },
    )
}

/// Where a panel's default stack stands.
fn status_of(target_id: Option<i64>, preview: Option<&SkyPreview>) -> &'static str {
    match (target_id, preview) {
        (None, _) => "missing_target",
        (Some(_), None) => "no_stack",
        (Some(_), Some(preview)) if preview.wcs.is_some() => "ready",
        (Some(_), Some(_)) => "unsolved",
    }
}

pub(super) async fn get(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<Mosaic>>, Error> {
    let service = enabled(&state)?;
    let catalogs: Vec<Arc<DatabaseContext>> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let mosaic = service
        .clone()
        .with_reader(move |store| {
            let project = store.project(id)?.ok_or(Error::Missing)?;
            let framing = store.framing_draft(id)?;
            let framing_revision = framing.as_ref().map(|draft| draft.revision);
            // Only a change activation writes moves the rectangles off the stacks.
            let layout_revision = framing.as_ref().map(|draft| draft.layout_revision);
            let activation = store.activation(id)?;
            let framing_stale = activation.as_ref().is_some_and(|activation| {
                layout_revision.is_some_and(|revision| revision > activation.framing_revision)
            });
            // One stack index read per rig database, however many panels it owns.
            let mut stacks: BTreeMap<String, std::collections::HashMap<i32, Vec<SkyPreview>>> =
                BTreeMap::new();
            let mut panels = Vec::new();
            let mut warnings = Vec::new();
            let found = identified_catalogs(&catalogs, service.instance_id);
            warnings.extend(found.duplicates.iter().cloned());
            let open = |context: &DatabaseContext| -> Result<rusqlite::Connection, Error> {
                let connection =
                    super::super::database_context::open_scheduler_connection_with_flags(
                        FilePath::new(&context.database_path),
                        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
                    )
                    .map_err(StoreError::from)?;
                connection
                    .busy_timeout(Duration::from_secs(2))
                    .map_err(StoreError::from)?;
                Ok(connection)
            };
            let mut panel = |rig: &NamedIdentity,
                             context: &DatabaseContext,
                             panel_id: String,
                             target_guid: Option<Uuid>,
                             row: Option<(i64, String)>,
                             progress: Option<PanelProgress>| {
                let found = stacks
                    .entry(context.id.clone())
                    .or_insert_with(|| stacks_for(context));
                let (target_id, target_name) = match row {
                    Some((target_id, name)) => (Some(target_id), Some(name)),
                    None => (None, None),
                };
                let list = target_id
                    .and_then(|target_id| i32::try_from(target_id).ok())
                    .and_then(|target_id| found.get(&target_id).cloned())
                    .unwrap_or_default();
                let preview = list.first().cloned();
                MosaicPanel {
                    panel_id,
                    rig: rig.clone(),
                    catalog_slug: Some(context.id.clone()),
                    catalog_name: context.name.clone(),
                    target_guid,
                    target_id,
                    target_name,
                    progress,
                    status: status_of(target_id, preview.as_ref()),
                    preview,
                    stacks: list,
                }
            };
            let mut reached = std::collections::BTreeSet::new();
            for activated in activation.iter().flat_map(|a| a.rigs.iter()) {
                reached.insert(activated.rig_id);
                let rig = store.rig(activated.rig_id)?.ok_or(Error::Missing)?;
                let Some(context) = found.get(activated.catalog_id) else {
                    warnings.push(format!(
                        "{}: its database is no longer registered on this server.",
                        rig.name
                    ));
                    for target in &activated.targets {
                        panels.push(MosaicPanel {
                            panel_id: target.panel_id.clone(),
                            rig: rig.clone(),
                            catalog_slug: None,
                            catalog_name: String::new(),
                            target_guid: Some(target.target_guid),
                            target_id: None,
                            target_name: None,
                            progress: None,
                            status: "missing_catalog",
                            preview: None,
                            stacks: vec![],
                        });
                    }
                    continue;
                };
                let connection = open(context)?;
                for target in &activated.targets {
                    let row =
                        target_row(&connection, target.target_guid).map_err(StoreError::from)?;
                    let plans: Vec<String> = activated
                        .plans
                        .iter()
                        .filter(|plan| plan.target_guid == target.target_guid)
                        .map(|plan| plan.exposureplan_guid.to_string())
                        .collect();
                    let progress = row
                        .as_ref()
                        .map(|(target_id, _)| progress(&connection, *target_id, &plans))
                        .transpose()
                        .map_err(StoreError::from)?;
                    panels.push(panel(&rig, context, target.panel_id.clone(), Some(target.target_guid), row, progress));
                }
            }
            // A rig no activation reached shows its linked project's targets.
            for (identity, context) in found.iter() {
                let Some(binding) = store.catalog_rig(identity.id)? else {
                    continue;
                };
                if reached.contains(&binding.rig.id) {
                    continue;
                }
                let mut after = None;
                let mut linked = Vec::new();
                loop {
                    let page = store.catalog_project_mappings(identity.id, after, 256)?;
                    linked.extend(page.items.iter().filter(|m| m.project_id == id).map(|m| m.source_project_guid));
                    match page.next_after {
                        Some(next) => after = Some(next),
                        None => break,
                    }
                }
                if linked.is_empty() {
                    continue;
                }
                let connection = match open(context) {
                    Ok(connection) => connection,
                    Err(_) => {
                        warnings.push(format!("{}: its database could not be read.", binding.rig.name));
                        continue;
                    }
                };
                for source in linked {
                    for (target_id, name, guid) in project_targets(&connection, source).map_err(StoreError::from)? {
                        let progress = target_progress(&connection, target_id).map_err(StoreError::from)?;
                        panels.push(panel(&binding.rig, context, name.clone(), guid, Some((target_id, name)), Some(progress)));
                    }
                }
            }
            if panels.is_empty() && activation.is_none() {
                warnings.push(
                    "Link a Target Scheduler project or activate the plan; stacks appear once the rigs have shot their panels."
                        .into(),
                );
            }
            panels.sort_by(|a, b| {
                a.panel_id
                    .cmp(&b.panel_id)
                    .then_with(|| a.rig.name.cmp(&b.rig.name))
            });
            Ok::<_, Error>(Mosaic {
                project,
                activation_revision: activation.as_ref().map(|a| a.revision),
                framing_revision,
                framing_stale,
                panels,
                warnings,
            })
        })
        .await?;
    Ok(Json(ApiResponse::success(mosaic)))
}
