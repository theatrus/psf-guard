//! Delete a project from a catalog, from the Library's Edit project panel.
//! See docs/design/project-removal.md.
//!
//! `GET …/projects/{id}/removal` previews what goes; `POST` with the
//! preview's digest deletes it. Neither runs as a job: the deletion is one
//! transaction of set-based statements, like a project merge, and the cache
//! files deleted after it belong to the project's frames. Deleting needs
//! database management and an editor's account.

use crate::commands::{project_removal, reject_removal};
use crate::server::api::ApiResponse;
use crate::server::database_context::{
    open_scheduler_connection, open_scheduler_connection_with_flags, DatabaseContext,
};
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
use crate::server::state::AppState;
use axum::extract::{Path, State};
use axum::Json;
use psf_guard_director_meta::merge::SourcePlan;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::sync::Arc;
use uuid::Uuid;

/// The plan that links the project, as the preview shows it.
#[derive(Debug, Clone, Serialize)]
pub struct LinkedPlan {
    pub name: String,
    /// Databases the plan links, this one included.
    pub databases: u32,
    /// The plan came from a collaboration. Its projects cannot be deleted.
    pub from_collaboration: bool,
}

impl From<SourcePlan> for LinkedPlan {
    fn from(link: SourcePlan) -> Self {
        Self {
            name: link.plan.name,
            databases: link.links,
            from_collaboration: link.imported,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RemovalPreview {
    #[serde(flatten)]
    pub plan: project_removal::RemovalPlan,
    pub linked_plan: Option<LinkedPlan>,
    /// The Target Scheduler database this catalog copies from, which keeps
    /// the project.
    pub synced_from: Option<String>,
    /// Database management is on, so this server can delete the project.
    pub allowed: bool,
}

#[derive(Debug, Deserialize)]
pub struct RemovalRequest {
    pub digest: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum RemovalOutcome {
    Removed {
        name: String,
        targets: usize,
        exposure_plans: usize,
        frames: usize,
        /// The plan that no longer links this database.
        plan: Option<String>,
        /// Planning could not forget the project; the plan still lists it.
        plan_error: Option<String>,
    },
    /// The project changed since the preview, so nothing was deleted.
    Changed { preview: RemovalPreview },
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// GUIDs of frames that a collaboration capture or report refers to. Their
/// project cannot be deleted.
async fn protected(state: &AppState) -> Result<BTreeSet<String>, AppError> {
    match state.director.clone() {
        Some(service) => service.reported_image_guids().await.map_err(|error| {
            AppError::InternalError(format!("reading collaboration reports: {error}"))
        }),
        None => Ok(BTreeSet::new()),
    }
}

/// The plan that links the project, read only when the project's GUID is
/// unique in the catalog.
async fn linked_plan(
    state: &AppState,
    ctx: &DatabaseContext,
    plan: &project_removal::RemovalPlan,
) -> Result<Option<LinkedPlan>, AppError> {
    let (Some(service), Some(guid)) = (state.director.clone(), planning_guid(plan)) else {
        return Ok(None);
    };
    service
        .source_project_plan(state.all_databases(), ctx.id.clone(), guid)
        .await
        .map(|link| link.map(LinkedPlan::from))
        .map_err(|error| AppError::InternalError(format!("reading the project's plan: {error}")))
}

fn planning_guid(plan: &project_removal::RemovalPlan) -> Option<Uuid> {
    plan.guid
        .as_deref()
        .filter(|_| plan.unique_guid)
        .and_then(|guid| Uuid::parse_str(guid).ok())
}

/// Plan the deletion on a read-only connection.
async fn read_plan(
    ctx: &DbContext,
    project_id: i64,
    protected: BTreeSet<String>,
) -> Result<project_removal::RemovalPlan, AppError> {
    let context = ctx.0.clone();
    tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &context.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(AppError::db)?;
        project_removal::plan(&conn, project_id, &protected).map_err(AppError::db)
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))??
    .ok_or(AppError::NotFound)
}

async fn preview_of(
    state: &AppState,
    ctx: &DatabaseContext,
    plan: project_removal::RemovalPlan,
) -> Result<RemovalPreview, AppError> {
    let linked_plan = linked_plan(state, ctx, &plan).await?;
    Ok(RemovalPreview {
        plan,
        linked_plan,
        synced_from: ctx
            .ts_sync
            .as_ref()
            .map(|sync| sync.source_path.clone())
            .filter(|path| !path.is_empty()),
        allowed: state.database_management_allowed(),
    })
}

/// `GET /api/db/{db}/projects/{project_id}/removal`
pub async fn preview(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, project_id)): Path<(String, i64)>,
) -> Result<Json<ApiResponse<RemovalPreview>>, AppError> {
    let protected = protected(&state).await?;
    let plan = read_plan(&ctx, project_id, protected).await?;
    Ok(Json(ApiResponse::success(
        preview_of(&state, &ctx, plan).await?,
    )))
}

/// `POST /api/db/{db}/projects/{project_id}/removal` with `{digest}`.
pub async fn remove(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, project_id)): Path<(String, i64)>,
    Json(request): Json<RemovalRequest>,
) -> Result<Json<ApiResponse<RemovalOutcome>>, AppError> {
    require_database_management_allowed(&state)?;
    let protected = protected(&state).await?;
    // A plan from a collaboration keeps its projects: the coordinator
    // assigned them. Checked before anything is deleted.
    let before = read_plan(&ctx, project_id, BTreeSet::new()).await?;
    let linked = linked_plan(&state, &ctx, &before).await?;
    if linked.as_ref().is_some_and(|plan| plan.from_collaboration) {
        return Err(AppError::Conflict(
            "This project belongs to a plan from a collaboration, so it cannot be deleted here."
                .into(),
        ));
    }

    let context = ctx.0.clone();
    let outcome = tokio::task::spawn_blocking(move || {
        let _organization = context.organization_mutex.lock().map_err(AppError::db)?;
        let conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        let outcome =
            project_removal::remove(&conn, project_id, &request.digest, &protected, now())
                .map_err(AppError::db)?;
        if let project_removal::Outcome::Removed { frames, .. } = &outcome {
            after_removal(&context, &conn, project_id, frames);
        }
        Ok::<_, AppError>(outcome)
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))??;

    let (plan, frames) = match outcome {
        project_removal::Outcome::Removed { plan, frames } => (plan, frames),
        project_removal::Outcome::Changed(plan) => {
            return Ok(Json(ApiResponse::success(RemovalOutcome::Changed {
                preview: preview_of(&state, &ctx, plan).await?,
            })));
        }
        project_removal::Outcome::Protected(plan) => {
            return Err(AppError::Conflict(format!(
                "{} of this project's frames were reported to a collaboration, so it cannot be deleted.",
                plan.protected_frames
            )));
        }
        project_removal::Outcome::Missing => return Err(AppError::NotFound),
    };
    tracing::info!(
        db = %ctx.id,
        project_id,
        name = %plan.name,
        targets = plan.targets,
        frames = frames.len(),
        "Deleted a project"
    );
    state.auto_stacks.touch_database(
        &ctx.id,
        crate::server::stack_preview::automatic::RefreshReason::Sync,
    );

    // Planning forgets the project after the rows are gone: forgotten first,
    // a refused removal would leave a project the next plan listing adopts
    // into a new plan.
    let named = linked.map(|plan| plan.name);
    let (linked, plan_error) = match (state.director.clone(), planning_guid(&plan)) {
        (Some(service), Some(guid)) => match service
            .forget_source_project(state.all_databases(), ctx.id.clone(), guid)
            .await
        {
            Ok(Some(_)) => (named, None),
            Ok(None) => (None, None),
            Err(error) => {
                tracing::warn!(db = %ctx.id, project_id, "Deleted a project but its plan still links it: {error}");
                (named, Some(error))
            }
        },
        _ => (None, None),
    };
    Ok(Json(ApiResponse::success(RemovalOutcome::Removed {
        name: plan.name,
        targets: plan.targets,
        exposure_plans: plan.exposure_plans,
        frames: frames.len(),
        plan: linked,
        plan_error,
    })))
}

/// What the server keeps about a deleted project: its frames' cache files
/// and pixel evidence, its stack indices, the file lookups and the
/// navigation counts. Target Scheduler gives a deleted row's Id to the next
/// row, which must not inherit any of them.
pub(crate) fn after_removal(
    context: &DatabaseContext,
    conn: &rusqlite::Connection,
    project_id: i64,
    frames: &[project_removal::RemovedFrame],
) {
    let removed: Vec<reject_removal::RemovedFrame> = frames
        .iter()
        .map(|frame| reject_removal::RemovedFrame {
            image_id: frame.image_id,
            guid: String::new(),
            project_id: Some(frame.project_id),
            target_id: frame.target_id,
        })
        .collect();
    reject_removal::forget_image_caches(&context.cache_dir_path, &removed);
    for frame in frames {
        if let Ok(id) = i32::try_from(frame.image_id) {
            crate::server::spatial_scan::invalidate_image_source(
                &context.spatial_metrics,
                &context.cache_dir_path,
                id,
            );
        }
    }
    if let Ok(id) = i32::try_from(project_id) {
        crate::server::stack_preview::forget_project_stacks(context, id);
    }
    context.file_check_cache.write().unwrap().clear();
    let _ = context.force_directory_tree_refresh();
    if let Err(error) = context.refresh_organization_navigation(conn) {
        tracing::warn!(db = %context.id, "Deleted a project but navigation refresh failed: {error}");
    }
}

/// `GET /api/db/{db}/deleted-projects`: deleted projects that syncs and
/// imports leave out, newest first.
pub async fn deleted(
    ctx: DbContext,
) -> Result<Json<ApiResponse<Vec<project_removal::Deletion>>>, AppError> {
    let context = ctx.0.clone();
    let deletions = tokio::task::spawn_blocking(move || {
        let conn = open_scheduler_connection_with_flags(
            &context.database_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(AppError::db)?;
        project_removal::deletions(&conn).map_err(AppError::db)
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))??;
    Ok(Json(ApiResponse::success(deletions)))
}

/// `POST /api/db/{db}/deleted-projects/{id}/allow`: let the next sync or
/// import add the project back.
pub async fn allow_back(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db, id)): Path<(String, i64)>,
) -> Result<Json<ApiResponse<serde_json::Value>>, AppError> {
    require_database_management_allowed(&state)?;
    let context = ctx.0.clone();
    let allowed = tokio::task::spawn_blocking(move || {
        let _organization = context.organization_mutex.lock().map_err(AppError::db)?;
        let conn = open_scheduler_connection(&context.database_path).map_err(AppError::db)?;
        project_removal::allow_back(&conn, id, now()).map_err(AppError::db)
    })
    .await
    .map_err(|error| AppError::InternalError(error.to_string()))??;
    if !allowed {
        return Err(AppError::NotFound);
    }
    tracing::info!(db = %ctx.id, id, "Allowed a deleted project back");
    Ok(Json(ApiResponse::success(
        serde_json::json!({ "allowed": true }),
    )))
}
