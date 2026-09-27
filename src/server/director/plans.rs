//! The Director page's plan list: every global project with where it is
//! linked, and how far its framing, plan and activation have come. Read only.

use super::rig_profile;
use super::*;
use crate::catalog_identity;
use crate::server::database_context::DatabaseContext;
use psf_guard_director_core::framing::FramingRequest;
use psf_guard_director_meta::{catalog::ProjectMapping, profile::RigProfile, CatalogIdentity};
use rusqlite::{OpenFlags, TransactionBehavior};
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
fn ensure_adopted(
    store: &mut MetaStore,
    instance: Uuid,
    catalog: &DatabaseContext,
) -> Result<(), String> {
    let mut connection = super::super::database_context::open_scheduler_connection_with_flags(
        FilePath::new(&catalog.database_path),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| format!("{}: could not be opened for planning", catalog.name))?;
    connection
        .busy_timeout(Duration::from_secs(2))
        .map_err(|_| format!("{}: busy", catalog.name))?;
    let mut tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
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
    let require_new = saved.is_none();
    let identity = saved.unwrap_or(CatalogIdentity {
        id: Uuid::new_v4(),
        origin_instance_id: instance,
    });
    let binding = match store.catalog_rig(identity.id) {
        Ok(Some(binding)) => binding,
        Ok(None) => store
            .bind_catalog_rig_after(identity, &catalog.name, require_new, || {
                catalog_identity::adopt(&mut tx, identity).map_err(|_| StoreError::Conflict)?;
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

pub(super) async fn list(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<PlanList>>, Error> {
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
    let list = tokio::task::spawn_blocking(move || {
        let _permits = (metadata_permit, catalog_permit);
        let mut store = service.store.lock().map_err(|_| Error::Internal)?;
        let mut warnings = Vec::new();
        for catalog in &catalogs {
            if let Err(warning) = ensure_adopted(&mut store, service.instance_id, catalog) {
                warnings.push(warning);
            }
        }
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
        Ok::<_, Error>(PlanList { rows, warnings })
    })
    .await
    .map_err(|error| {
        tracing::error!(%error, "Director plan listing failed");
        Error::Internal
    })??;
    Ok(Json(ApiResponse::success(list)))
}
