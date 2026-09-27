//! The Director page's plan list: every global project with where it is
//! linked, and how far its framing, plan and activation have come. Read only.

use super::*;
use psf_guard_director_core::framing::FramingRequest;
use rusqlite::OpenFlags;
use std::{collections::BTreeMap, time::Duration};

#[derive(Serialize)]
struct PlanLink {
    catalog_slug: String,
    catalog_name: String,
    rig: NamedIdentity,
    source_project_guid: Uuid,
    /// The row and name in that database, when the project still exists there.
    source_row_id: Option<i64>,
    source_name: Option<String>,
}

#[derive(Serialize)]
struct FramingSummary {
    revision: u64,
    target_name: String,
    panels: u32,
    panel_rig_id: Option<Uuid>,
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
    framing: Option<FramingSummary>,
    plan: Option<PlanSummary>,
    activation: Option<ActivationSummary>,
}

const MAX_PROJECTS: usize = 1024;

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<PlanRow>>>, Error> {
    let service = enabled(&state)?;
    let catalogs: Vec<_> = state
        .databases
        .read()
        .map_err(|_| Error::Internal)?
        .values()
        .cloned()
        .collect();
    let metadata_permit = admit(&service.admission).await?;
    let catalog_permit = admit(&service.discovery_admission).await?;
    let rows = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let store = service.store.lock().map_err(|_| Error::Internal)?;
        // Links first: each bound database's mappings, joined to its rows.
        let mut links: BTreeMap<Uuid, Vec<PlanLink>> = BTreeMap::new();
        for catalog in &catalogs {
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
            let Some(identity) = crate::catalog_identity::read(&connection).ok().flatten() else {
                continue;
            };
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
            let mut after = None;
            loop {
                let page = store.catalog_project_mappings(identity.id, after, 256)?;
                for mapping in &page.items {
                    let row = rows.get(&mapping.source_project_guid);
                    links.entry(mapping.project_id).or_default().push(PlanLink {
                        catalog_slug: catalog.id.clone(),
                        catalog_name: catalog.name.clone(),
                        rig: binding.rig.clone(),
                        source_project_guid: mapping.source_project_guid,
                        source_row_id: row.map(|(id, _)| *id),
                        source_name: row.and_then(|(_, name)| name.clone()),
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
            let framing = store
                .framing_draft(project.id)?
                .map(|draft| FramingSummary {
                    revision: draft.revision,
                    target_name: draft.target_name.clone(),
                    panels: draft
                        .panel
                        .map(|panel| {
                            FramingRequest {
                                center: draft.center,
                                position_angle_degrees: draft.position_angle_degrees,
                                panel,
                                mosaic: draft.mosaic,
                                overlays: vec![],
                                view: None,
                            }
                            .preview()
                            .map(|p| p.panels.len() as u32)
                            .unwrap_or(0)
                        })
                        // No panel size yet; the grid still says how many.
                        .unwrap_or(draft.mosaic.rows * draft.mosaic.columns),
                    panel_rig_id: draft.panel_rig_id,
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
            rows.push(PlanRow {
                links: links.remove(&project.id).unwrap_or_default(),
                project,
                framing,
                plan,
                activation,
            });
        }
        Ok::<_, Error>(rows)
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director plan listing failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(rows)))
}
