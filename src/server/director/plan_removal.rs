//! Delete a plan with nothing captured: its project in each database it
//! links, then the plan. See docs/design/project-removal.md.
//!
//! `GET /projects/{id}/deletion` previews each database's project and why
//! the plan cannot be deleted, if it cannot. `POST` with each project's
//! digest deletes the projects, one database at a time, then the plan. A
//! rig that captured frames keeps the plan: those projects are deleted from
//! the Library, one database at a time, where the frames are counted.
//!
//! A plan from a collaboration leaves it in the same meta-store transaction
//! that deletes the plan, then the coordinator is told, as Leave
//! collaboration does (see `collaboration_leave`). Frames queued for or
//! sent in a report keep the plan until the person leaves on purpose.

use super::collaboration_auth::leave as remote;
use super::collaboration_leave::CollaborationView;
use super::*;
use crate::commands::project_removal;
use crate::server::auth::RequestAccess;
use crate::server::database_context::{
    open_scheduler_connection, open_scheduler_connection_with_flags, DatabaseContext,
};
use axum::{http::HeaderMap, Extension};
use rusqlite::OptionalExtension;
use std::collections::BTreeSet;

/// One database the plan links, and its project there.
#[derive(Clone, Debug, Serialize)]
pub(super) struct LinkedDatabase {
    /// The project's GUID, which the delete keys its digest by.
    source_project_guid: Uuid,
    catalog_slug: Option<String>,
    catalog_name: Option<String>,
    /// The Target Scheduler database this catalog copies from, which keeps
    /// the project.
    synced_from: Option<String>,
    /// The project in that database; `None` when its row is gone.
    project: Option<LinkedProject>,
    /// Why the database could not be read, when it could not.
    problem: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct LinkedProject {
    id: i64,
    name: String,
    /// Target Scheduler's state: 0 draft, 1 active, 2 inactive, 3 closed.
    state: Option<i64>,
    targets: usize,
    exposure_plans: usize,
    frames: usize,
    digest: String,
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct PlanDeletionPreview {
    name: String,
    databases: Vec<LinkedDatabase>,
    /// The collaboration the plan came from, which deleting leaves.
    collaboration: Option<CollaborationView>,
    /// Why the plan cannot be deleted, or `None` when it can.
    refusal: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct DeletionRequest {
    /// Each project's digest from the preview, by its GUID.
    #[serde(default)]
    digests: BTreeMap<Uuid, String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum DeletionOutcome {
    Deleted {
        name: String,
        /// Databases whose project was deleted.
        databases: Vec<String>,
        /// What telling the coordinator did, for a plan from a
        /// collaboration.
        #[serde(skip_serializing_if = "Option::is_none")]
        coordinator: Option<remote::Told>,
    },
    /// The plan or a project changed since the preview, so the plan stays.
    /// The databases in `deleted` lost their project already.
    Changed {
        preview: PlanDeletionPreview,
        deleted: Vec<String>,
    },
    /// Deleting stopped on an error. The databases in `deleted` lost their
    /// project already.
    Stopped { error: String, deleted: Vec<String> },
}

/// A database the plan links, as deleting its project needs it.
struct Target {
    catalog: Uuid,
    guid: Uuid,
    context: Option<Arc<DatabaseContext>>,
    row: Option<i64>,
}

/// The plan's name and each database it links, read from the files.
async fn read_preview(
    state: &Arc<AppState>,
    service: &Arc<Service>,
    id: Uuid,
) -> Result<(PlanDeletionPreview, Vec<Target>), Error> {
    let (plan, links, collaboration, linked) = service
        .clone()
        .query(move |store| {
            let plan = store.project(id)?;
            let mut links = Vec::new();
            for (catalog, guid) in store.plan_links(id)? {
                let rig = store.catalog_rig(catalog)?.map(|binding| binding.rig.name);
                links.push((catalog, guid, rig));
            }
            let collaboration = store.plan_collaboration(id)?;
            let linked = super::collaboration_leave::linked_databases(store, id)?;
            Ok((plan, links, collaboration, linked))
        })
        .await?;
    let plan = plan.ok_or(Error::Missing)?;
    let catalogs = state.all_databases();
    let instance = service.instance_id;
    let imports = collaboration
        .as_ref()
        .map(|collaboration| collaboration.import_ids.clone())
        .unwrap_or_default();
    let (read, open) = blocking(move || {
        let identified = identified_catalogs(&catalogs, instance);
        // Deleting the projects deletes their assignments; the count is
        // for the dialog. A database that cannot be read is a problem the
        // links below report.
        let open = super::collaboration_leave::read_assignments(&identified, linked, &imports)
            .map(|found| found.iter().map(|(_, open)| open).sum())
            .unwrap_or(0);
        let read = links
            .into_iter()
            .map(|(catalog, guid, rig)| read_link(&identified, catalog, guid, rig))
            .collect::<Vec<_>>();
        (read, open)
    })
    .await?;
    let (databases, targets): (Vec<_>, Vec<_>) = read.into_iter().unzip();
    let collaboration = match &collaboration {
        Some(collaboration) => Some(
            super::collaboration_leave::view(state, service.clone(), collaboration, open).await,
        ),
        None => None,
    };
    let refusal = if let Some(problem) = databases
        .iter()
        .find_map(|database| database.problem.clone())
    {
        Some(problem)
    } else if let Some(reported) = collaboration
        .as_ref()
        .filter(|collaboration| collaboration.reported_frames > 0)
    {
        Some(format!(
            "{} of this plan {} reported to the collaboration at {}. Open the plan and choose Leave collaboration under Collaboration assignments, then delete the plan.",
            count(reported.reported_frames as usize, "frame"),
            if reported.reported_frames == 1 { "was" } else { "were" },
            reported.host
        ))
    } else {
        databases.iter().find_map(|database| {
            let project = database
                .project
                .as_ref()
                .filter(|project| project.frames > 0)?;
            Some(format!(
                "{} has {} frame{} of this plan. Delete that project from the Library first.",
                database.catalog_name.as_deref().unwrap_or("A database"),
                project.frames,
                if project.frames == 1 { "" } else { "s" }
            ))
        })
    };
    Ok((
        PlanDeletionPreview {
            name: plan.name,
            databases,
            collaboration,
            refusal,
        },
        targets,
    ))
}

fn count(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// Read one link's project from its database file.
fn read_link(
    identified: &IdentifiedCatalogs,
    catalog: Uuid,
    guid: Uuid,
    rig: Option<String>,
) -> (LinkedDatabase, Target) {
    let mut database = LinkedDatabase {
        source_project_guid: guid,
        catalog_slug: None,
        catalog_name: None,
        synced_from: None,
        project: None,
        problem: None,
    };
    let mut target = Target {
        catalog,
        guid,
        context: None,
        row: None,
    };
    let Some(context) = identified.get(catalog).cloned() else {
        database.problem = Some(match rig {
            Some(rig) => format!(
                "The database for {rig} is not registered on this server. Register it again in Settings, then delete the plan."
            ),
            None => "A database this plan links is not registered on this server. Register it again in Settings, then delete the plan.".into(),
        });
        return (database, target);
    };
    database.catalog_slug = Some(context.id.clone());
    database.catalog_name = Some(context.name.clone());
    database.synced_from = context
        .ts_sync
        .as_ref()
        .map(|sync| sync.source_path.clone())
        .filter(|path| !path.is_empty());
    let found = (|| -> anyhow::Result<Option<project_removal::RemovalPlan>> {
        let conn = open_scheduler_connection_with_flags(
            &context.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        let row: Option<i64> = conn
            .query_row(
                "SELECT Id FROM project WHERE lower(trim(guid)) = lower(?1) ORDER BY Id LIMIT 1",
                [guid.to_string()],
                |row| row.get(0),
            )
            .optional()?;
        let Some(row) = row else { return Ok(None) };
        project_removal::plan(&conn, row, &BTreeSet::new())
    })();
    match found {
        Ok(Some(plan)) if !plan.unique_guid => {
            database.problem = Some(format!(
                "{} has more than one project with this plan's GUID. Fix them in Target Scheduler, then delete the plan.",
                context.name
            ));
        }
        Ok(Some(plan)) => {
            target.row = Some(plan.project_id);
            database.project = Some(LinkedProject {
                id: plan.project_id,
                name: plan.name,
                state: plan.state,
                targets: plan.targets,
                exposure_plans: plan.exposure_plans,
                frames: plan.frames.total,
                digest: plan.digest,
            });
        }
        Ok(None) => {}
        Err(error) => {
            database.problem = Some(format!("{} could not be read: {error:#}", context.name));
        }
    }
    target.context = Some(context);
    (database, target)
}

/// `GET /projects/{id}/deletion`
pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<PlanDeletionPreview>>, Error> {
    let service = enabled(&state)?;
    let (preview, _) = read_preview(&state, &service, id).await?;
    Ok(Json(ApiResponse::success(preview)))
}

/// Forget the links of projects already deleted, so a plan that stays does
/// not list databases where its project is gone. False when it could not.
async fn forget(service: &Arc<Service>, gone: Vec<(Uuid, Uuid)>) -> bool {
    if gone.is_empty() {
        return true;
    }
    match service
        .clone()
        .run(move |store| {
            for (catalog, guid) in gone {
                store.forget_source_project(catalog, guid)?;
            }
            Ok(())
        })
        .await
    {
        Ok(()) => true,
        Err(error) => {
            tracing::warn!(
                ?error,
                "Deleted a plan's projects but could not forget their links"
            );
            false
        }
    }
}

const STILL_LISTED: &str =
    "The plan still lists the databases whose project was deleted; delete the plan again.";

fn stopped(
    error: String,
    deleted: Vec<String>,
) -> Result<Json<ApiResponse<DeletionOutcome>>, Error> {
    Ok(Json(ApiResponse::success(DeletionOutcome::Stopped {
        error,
        deleted,
    })))
}

/// `POST /projects/{id}/deletion` with each project's digest.
pub(super) async fn delete(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    headers: HeaderMap,
    Json(request): Json<DeletionRequest>,
) -> Result<Json<ApiResponse<DeletionOutcome>>, Error> {
    let service = writable(&state)?;
    // The collaboration gate first, as nightly intake takes it before it
    // activates work: no intake, report or collaboration operation runs
    // half way through. Then the turn at the rig databases that discovery,
    // plan listings and activation take, and the sync lock activation
    // holds: none of them writes this plan's rigs or links while it is
    // deleted.
    let _gate = remote::hold(&service).await?;
    let catalogs = admit(&service.discovery_admission).await?;
    let sync = state.sync_apply_lock.lock().await;
    let (preview, targets) = read_preview(&state, &service, id).await?;
    if preview.collaboration.is_some() {
        // Deleting leaves the collaboration and signs in to the coordinator
        // as joining does.
        remote::interactive(&state, &access, &headers).map_err(Error::Denied)?;
    }
    if let Some(refusal) = preview.refusal.clone() {
        return Err(Error::Refused(refusal));
    }
    let changed = |preview, deleted| {
        Ok(Json(ApiResponse::success(DeletionOutcome::Changed {
            preview,
            deleted,
        })))
    };
    // Every project needs the digest of the preview the person saw before
    // anything is deleted.
    let missing = preview.databases.iter().any(|database| {
        database.project.is_some() && !request.digests.contains_key(&database.source_project_guid)
    });
    if missing {
        return changed(preview, Vec::new());
    }
    let links: Vec<(Uuid, Uuid)> = targets
        .iter()
        .map(|target| (target.catalog, target.guid))
        .collect();
    let mut deleted = Vec::new();
    let mut gone: Vec<(Uuid, Uuid)> = Vec::new();
    for (target, database) in targets.into_iter().zip(&preview.databases) {
        let (Some(context), Some(row), Some(project)) =
            (target.context, target.row, database.project.as_ref())
        else {
            continue;
        };
        let digest = request.digests[&target.guid].clone();
        let removing = Arc::clone(&context);
        let outcome = blocking(move || -> Result<project_removal::Outcome, String> {
            let _organization = removing
                .organization_mutex
                .lock()
                .map_err(|error| error.to_string())?;
            let conn = open_scheduler_connection(&removing.database_path)
                .map_err(|error| error.to_string())?;
            let outcome =
                project_removal::remove(&conn, row, &digest, &BTreeSet::new(), now_seconds())
                    .map_err(|error| format!("{error:#}"))?;
            if let project_removal::Outcome::Removed { frames, .. } = &outcome {
                crate::server::project_removal::after_removal(&removing, &conn, row, frames);
            }
            Ok(outcome)
        })
        .await?;
        match outcome {
            Ok(project_removal::Outcome::Removed { .. } | project_removal::Outcome::Missing) => {
                state.auto_stacks.touch_database(
                    &context.id,
                    crate::server::stack_preview::automatic::RefreshReason::Sync,
                );
                deleted.push(context.name.clone());
                gone.push((target.catalog, target.guid));
            }
            Ok(_) => {
                if !forget(&service, gone).await {
                    return stopped(STILL_LISTED.into(), deleted);
                }
                let (fresh, _) = read_preview(&state, &service, id).await?;
                return changed(fresh, deleted);
            }
            Err(error) => {
                let mut error = format!(
                    "Could not delete {} from {}: {error}",
                    project.name, context.name
                );
                if !forget(&service, gone).await {
                    error.push(' ');
                    error.push_str(STILL_LISTED);
                }
                return stopped(error, deleted);
            }
        }
    }
    match service
        .clone()
        .run(move |store| store.delete_plan(id, &links))
        .await
    {
        Ok(left) => {
            tracing::info!(plan = %id, name = %preview.name, databases = deleted.len(), "Deleted a plan");
            drop(sync);
            drop(catalogs);
            let coordinator = match left {
                Some(left) => {
                    remote::forget_runs(service.clone(), left.base_url.clone()).await;
                    Some(
                        remote::decline(&state, service.clone(), &left.base_url, &left.tasks).await,
                    )
                }
                None => None,
            };
            Ok(Json(ApiResponse::success(DeletionOutcome::Deleted {
                name: preview.name,
                databases: deleted,
                coordinator,
            })))
        }
        // The plan took a link, or reported frames, meanwhile.
        Err(Error::Conflict) => {
            if !forget(&service, gone).await {
                return stopped(STILL_LISTED.into(), deleted);
            }
            let (fresh, _) = read_preview(&state, &service, id).await?;
            changed(fresh, deleted)
        }
        Err(error) => {
            tracing::warn!(plan = %id, ?error, "Deleted a plan's projects but not the plan");
            forget(&service, gone).await;
            stopped(
                "The projects were deleted, but the plan could not be. Delete the plan again."
                    .into(),
                deleted,
            )
        }
    }
}

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}
