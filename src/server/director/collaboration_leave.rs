//! Leave a collaboration: the plan an import created becomes an ordinary
//! local plan. See docs/design/collaboration.md, "Leaving a collaboration".
//!
//! `GET /projects/{id}/collaboration/leave` previews what leaving removes
//! and keeps. `POST` ends the imports' open assignments in the rig
//! databases the plan links, removes the collaboration records in one
//! meta-store transaction, then declines the plan's tasks at the
//! coordinator. The plan, its rig projects, frames, grades and drafts stay.
//!
//! Locks, in the order every holder takes them: the collaboration gate
//! (nightly intake, automatic reports, report delivery and interactive
//! collaboration work), the rig-database turn (`discovery_admission`), the
//! sync lock, then each database's organization lock and the meta writer.
//! Nightly intake activates work while it holds the gate, so the gate must
//! come first.

use super::collaboration_auth::leave as remote;
use super::*;
use crate::server::auth::RequestAccess;
use crate::server::database_context::{
    open_scheduler_connection, open_scheduler_connection_with_flags, DatabaseContext,
};
use axum::{http::HeaderMap, Extension};
use psf_guard_director_meta::collaboration_leave::PlanCollaboration;

/// The collaboration a plan came from, as the leave and delete dialogs
/// show it.
#[derive(Clone, Debug, Serialize)]
pub(super) struct CollaborationView {
    /// The collaboration server's host, such as `collab.starfront.space`.
    pub(super) host: String,
    pub(super) base_url: String,
    pub(super) remote_project_id: String,
    pub(super) remote_project_name: Option<String>,
    /// Nightly assignments imported for the plan.
    pub(super) imports: usize,
    /// Reports not yet acknowledged by the coordinator.
    pub(super) unsent_reports: u32,
    /// Reports the coordinator acknowledged.
    pub(super) sent_reports: u32,
    /// Frames queued for or sent in a report.
    pub(super) reported_frames: u32,
    /// File transfers for the imports that have not finished.
    pub(super) transfers: u32,
    /// Connections whose nightly intake lists the remote project.
    pub(super) nightly_intake: Vec<String>,
    /// Assignments still open in the rig databases.
    pub(super) open_assignments: usize,
    /// Every task's rig has a registered connection with a saved sign-in,
    /// so leaving can decline the tasks at the coordinator.
    pub(super) coordinator_reachable: bool,
}

pub(super) async fn view(
    state: &AppState,
    service: Arc<Service>,
    collaboration: &PlanCollaboration,
    open_assignments: usize,
) -> CollaborationView {
    let coordinator_reachable = remote::reachable(
        state,
        service,
        collaboration.base_url.clone(),
        &collaboration.tasks,
    )
    .await;
    CollaborationView {
        host: remote::host(&collaboration.base_url),
        base_url: collaboration.base_url.clone(),
        remote_project_id: collaboration.remote_project_id.clone(),
        remote_project_name: collaboration.remote_project_name.clone(),
        imports: collaboration.import_ids.len(),
        unsent_reports: collaboration.unsent_reports,
        sent_reports: collaboration.sent_reports,
        reported_frames: collaboration.reported_frames,
        transfers: collaboration.transfers,
        nightly_intake: collaboration
            .nightly_intake
            .iter()
            .map(|intake| intake.name.clone())
            .collect(),
        open_assignments,
        coordinator_reachable,
    }
}

#[derive(Clone, Debug, Serialize)]
pub(super) struct LeavePreview {
    name: String,
    /// `None` for a plan of its own.
    collaboration: Option<CollaborationView>,
    /// Linked databases leaving cannot reach: their assignments stay open.
    warnings: Vec<String>,
    /// Why the plan cannot leave, or `None` when it can.
    refusal: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(super) enum LeaveOutcome {
    Left {
        name: String,
        collaboration: CollaborationView,
        /// Open assignments ended in the rig databases.
        assignments_ended: usize,
        /// Linked databases whose assignments could not be ended.
        warnings: Vec<String>,
        coordinator: remote::Told,
    },
}

/// Each database a plan links, once, with its rig's name.
pub(super) fn linked_databases(
    store: &MetaStore,
    plan: Uuid,
) -> Result<Vec<(Uuid, Option<String>)>, StoreError> {
    let mut databases = Vec::new();
    for (catalog, _) in store.plan_links(plan)? {
        if databases.iter().any(|(seen, _)| *seen == catalog) {
            continue;
        }
        let rig = store.catalog_rig(catalog)?.map(|binding| binding.rig.name);
        databases.push((catalog, rig));
    }
    Ok(databases)
}

/// What leaving does in a database it cannot reach: the warning the dialog
/// and the result show.
fn unreachable(database: &str, why: &str) -> String {
    format!(
        "{database} {why}, so the plan's assignments there stay open. PSF Guard no longer counts \
         frames toward them. To end them, register the database again in Settings if it is \
         missing, then activate the plan."
    )
}

/// The linked databases that can be read, with how many of the imports'
/// assignments are open in each, and a warning for each one that cannot.
pub(super) fn read_assignments(
    identified: &IdentifiedCatalogs,
    databases: Vec<(Uuid, Option<String>)>,
    imports: &[Uuid],
) -> (Vec<(Arc<DatabaseContext>, usize)>, Vec<String>) {
    let imports = serde_json::to_string(imports).unwrap_or_else(|_| "[]".into());
    let mut found = Vec::new();
    let mut warnings = Vec::new();
    for (catalog, rig) in databases {
        let Some(context) = identified.get(catalog).cloned() else {
            warnings.push(unreachable(
                &match rig {
                    Some(rig) => format!("The database for {rig}"),
                    None => "A database this plan links".into(),
                },
                "is not registered on this server",
            ));
            continue;
        };
        let open = (|| -> rusqlite::Result<usize> {
            let conn = open_scheduler_connection_with_flags(
                &context.database_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
            )
            .map_err(|_| rusqlite::Error::InvalidQuery)?;
            open_assignments(&conn, &imports)
        })();
        match open {
            Ok(open) => found.push((context, open)),
            Err(error) => warnings.push(unreachable(
                &context.name,
                &format!("could not be read ({error})"),
            )),
        }
    }
    (found, warnings)
}

fn has_table(conn: &rusqlite::Connection) -> rusqlite::Result<bool> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [collaboration_activation::TABLE],
        |row| row.get(0),
    )
}

/// Open assignments of `imports`, a JSON array of import IDs.
fn open_assignments(conn: &rusqlite::Connection, imports: &str) -> rusqlite::Result<usize> {
    if !has_table(conn)? {
        return Ok(0);
    }
    let open: i64 = conn.query_row(
        "SELECT count(*) FROM psf_guard_collaboration_plan
         WHERE end_at_ms IS NULL AND import_id IN (SELECT value FROM json_each(?1))",
        [imports],
        |row| row.get(0),
    )?;
    Ok(usize::try_from(open).unwrap_or(0))
}

/// End the imports' open assignments in one database, at `now` in whole
/// seconds as activation's cutovers are. The rows stay as a record of what
/// the coordinator assigned; no frame after `now` counts toward them.
fn end_assignments(
    context: &DatabaseContext,
    imports: &[Uuid],
    now_ms: u64,
) -> Result<usize, String> {
    let imports = serde_json::to_string(imports).map_err(|error| error.to_string())?;
    let _organization = context
        .organization_mutex
        .lock()
        .map_err(|error| error.to_string())?;
    let conn =
        open_scheduler_connection(&context.database_path).map_err(|error| error.to_string())?;
    if !has_table(&conn).map_err(|error| error.to_string())? {
        return Ok(0);
    }
    conn.execute(
        "UPDATE psf_guard_collaboration_plan SET end_at_ms=?2
         WHERE end_at_ms IS NULL AND import_id IN (SELECT value FROM json_each(?1))",
        rusqlite::params![imports, (now_ms / 1000 * 1000) as i64],
    )
    .map_err(|error| error.to_string())
}

/// The plan's name, its collaboration and each linked database's open
/// assignments, read from the store and the files.
async fn read_preview(
    state: &Arc<AppState>,
    service: &Arc<Service>,
    id: Uuid,
) -> Result<
    (
        LeavePreview,
        Vec<Arc<DatabaseContext>>,
        Option<PlanCollaboration>,
    ),
    Error,
> {
    let (plan, collaboration, databases) = service
        .clone()
        .query(move |store| {
            Ok((
                store.project(id)?,
                store.plan_collaboration(id)?,
                linked_databases(store, id)?,
            ))
        })
        .await?;
    let plan = plan.ok_or(Error::Missing)?;
    let Some(collaboration) = collaboration else {
        return Ok((
            LeavePreview {
                name: plan.name,
                collaboration: None,
                warnings: Vec::new(),
                refusal: Some("This plan did not come from a collaboration.".into()),
            },
            Vec::new(),
            None,
        ));
    };
    let catalogs = state.all_databases();
    let instance = service.instance_id;
    let imports = collaboration.import_ids.clone();
    let (found, warnings) = blocking(move || {
        read_assignments(
            &identified_catalogs(&catalogs, instance),
            databases,
            &imports,
        )
    })
    .await?;
    let open = found.iter().map(|(_, open)| open).sum();
    let contexts = found.into_iter().map(|(context, _)| context).collect();
    let view = view(state, service.clone(), &collaboration, open).await;
    Ok((
        LeavePreview {
            name: plan.name,
            collaboration: Some(view),
            warnings,
            refusal: None,
        },
        contexts,
        Some(collaboration),
    ))
}

/// `GET /projects/{id}/collaboration/leave`
pub(super) async fn preview(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> Result<Json<ApiResponse<LeavePreview>>, Error> {
    let service = enabled(&state)?;
    let (preview, _, _) = read_preview(&state, &service, id).await?;
    Ok(Json(ApiResponse::success(preview)))
}

/// `POST /projects/{id}/collaboration/leave`
pub(super) async fn leave(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Extension(access): Extension<RequestAccess>,
    headers: HeaderMap,
) -> Result<Json<ApiResponse<LeaveOutcome>>, Error> {
    let service = writable(&state)?;
    // Leaving signs in to the coordinator as joining does.
    remote::interactive(&state, &access, &headers).map_err(Error::Denied)?;
    let _gate = remote::hold(&service).await?;
    let catalogs = admit(&service.discovery_admission).await?;
    let sync = state.sync_apply_lock.lock().await;
    let (preview, contexts, collaboration) = read_preview(&state, &service, id).await?;
    if let Some(refusal) = preview.refusal {
        return Err(Error::Refused(refusal));
    }
    let collaboration = collaboration.ok_or(Error::Missing)?;
    let imports = collaboration.import_ids.clone();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0);
    // A person can always leave: a database that cannot be written keeps
    // its open rows, and the result says so.
    let mut warnings = preview.warnings;
    let (ended, unwritten) = blocking(move || {
        let mut ended = 0;
        let mut unwritten = Vec::new();
        for context in contexts {
            match end_assignments(&context, &imports, now) {
                Ok(count) => ended += count,
                Err(error) => unwritten.push(unreachable(
                    &context.name,
                    &format!("could not be written ({error})"),
                )),
            }
        }
        (ended, unwritten)
    })
    .await?;
    for warning in &unwritten {
        tracing::warn!(plan = %id, %warning, "Left a collaboration with assignments still open");
    }
    warnings.extend(unwritten);
    let left = match service
        .clone()
        .run(move |store| store.leave_collaboration(id))
        .await
    {
        Ok(left) => left,
        Err(error) => {
            tracing::warn!(plan = %id, ?error, ended, "Ended a plan's assignments but did not leave its collaboration");
            return Err(error);
        }
    };
    drop(sync);
    drop(catalogs);
    let view = view(&state, service.clone(), &left, ended).await;
    remote::forget_runs(service.clone(), left.base_url.clone()).await;
    let coordinator = remote::decline(&state, service.clone(), &left.base_url, &left.tasks).await;
    tracing::info!(plan = %id, host = %view.host, declined = coordinator.declined.len(), "Left a collaboration");
    Ok(Json(ApiResponse::success(LeaveOutcome::Left {
        name: preview.name,
        collaboration: view,
        assignments_ended: ended,
        warnings,
        coordinator,
    })))
}
