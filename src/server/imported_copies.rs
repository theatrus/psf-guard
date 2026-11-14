//! A database's imported copies in Settings: lights catalogued twice, once
//! as Target Scheduler recorded them and once by an import of the same
//! file, and the merge that removes the copies and the empty projects and
//! targets the import made (#1100). See [`crate::imported_copies`].
//!
//! - **Check** (`GET …/imported-copies`) plans on its own read-only
//!   connection. It writes nothing.
//! - **Merge** (`POST …/imported-copies/merge` with the check's digest)
//!   copies the database beside itself, then plans again and applies in one
//!   transaction. When the catalog changed since the check it changes
//!   nothing and returns the new plan.

use std::sync::Arc;

use axum::{extract::State, Json};
use serde::{Deserialize, Serialize};

use crate::commands::reject_removal::RemovedFrame;
use crate::imported_copies::{self, FoldOutcome, FoldPlan, Keepers};
use crate::server::api::ApiResponse;
use crate::server::database_context::open_scheduler_connection;
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
use crate::server::state::AppState;

/// What the check found.
#[derive(Debug, Serialize)]
pub struct ImportedCopiesReport {
    #[serde(flatten)]
    pub plan: FoldPlan,
    /// Whether this server can write the database file.
    pub writable: bool,
}

#[derive(Debug, Deserialize)]
pub struct MergeRequest {
    pub digest: String,
}

/// How a merge ended.
#[derive(Debug, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum MergeOutcome {
    Merged {
        #[serde(flatten)]
        plan: FoldPlan,
        backup_path: Option<String>,
    },
    /// The catalog changed since the check, so nothing changed.
    Changed { preview: ImportedCopiesReport },
}

fn failed(error: impl std::fmt::Display) -> AppError {
    AppError::InternalError(format!("{error:#}"))
}

/// Frames a collaboration report names, and projects a plan links: none
/// without Director.
async fn keepers(state: &AppState) -> Result<Keepers, AppError> {
    let Some(service) = state.director.clone() else {
        return Ok(Keepers::default());
    };
    let reported = service
        .clone()
        .reported_image_guids()
        .await
        .map_err(|error| {
            AppError::InternalError(format!("reading collaboration reports: {error}"))
        })?;
    let planned_projects = service
        .linked_source_project_guids()
        .await
        .map_err(|error| AppError::InternalError(format!("reading Director plans: {error}")))?;
    Ok(Keepers {
        reported,
        planned_projects,
    })
}

/// `GET /api/db/{db_id}/imported-copies` — lights an import catalogued
/// beside Target Scheduler's records of them. Read only.
pub async fn report(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<ImportedCopiesReport>>, AppError> {
    let keepers = keepers(&state).await?;
    let ctx = Arc::clone(&ctx.0);
    let report = tokio::task::spawn_blocking(move || -> anyhow::Result<ImportedCopiesReport> {
        let path = std::path::Path::new(&ctx.database_path);
        let conn = crate::ts_guids::open_existing(path, false)?;
        Ok(ImportedCopiesReport {
            plan: imported_copies::plan(&conn, &keepers)?,
            writable: crate::ts_guids::is_writable(path),
        })
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    Ok(Json(ApiResponse::success(report)))
}

/// `POST /api/db/{db_id}/imported-copies/merge` with `{digest}` — copy the
/// database beside itself, then remove the copies and the empty projects
/// and targets. Runs off the request path on its own connection.
pub async fn merge(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<MergeRequest>,
) -> Result<Json<ApiResponse<MergeOutcome>>, AppError> {
    require_database_management_allowed(&state)?;
    let keepers = keepers(&state).await?;
    let db_id = ctx.id.clone();
    let ctx = Arc::clone(&ctx.0);
    let outcome = tokio::task::spawn_blocking(move || -> anyhow::Result<MergeOutcome> {
        let path = std::path::Path::new(&ctx.database_path);
        let repaired = {
            let _guard = ctx
                .organization_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("the catalog's organization lock is poisoned"))?;
            imported_copies::repair_database(path, Some(&request.digest), &keepers)?
        };
        let plan = match repaired.outcome {
            FoldOutcome::Changed(plan) => {
                return Ok(MergeOutcome::Changed {
                    preview: ImportedCopiesReport {
                        plan,
                        writable: crate::ts_guids::is_writable(path),
                    },
                });
            }
            FoldOutcome::Folded(plan) => plan,
        };
        if !plan.is_empty() {
            let conn = open_scheduler_connection(&ctx.database_path)?;
            let removed: Vec<RemovedFrame> = plan
                .removed()
                .map(|light| RemovedFrame {
                    image_id: light.id,
                    guid: light.guid.clone(),
                    project_id: light.project_id,
                    target_id: light.target_id,
                })
                .collect();
            crate::server::reject_removal::after_change(&ctx, &conn, &removed);
            for project in &plan.projects {
                crate::server::project_removal::after_removal(&ctx, &conn, project.id, &[]);
            }
        }
        Ok(MergeOutcome::Merged {
            plan,
            backup_path: repaired.backup.map(|backup| backup.display().to_string()),
        })
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    if let MergeOutcome::Merged {
        plan,
        backup_path: Some(backup),
    } = &outcome
    {
        tracing::info!(
            db = %db_id,
            groups = plan.groups.len(),
            projects = plan.projects.len(),
            targets = plan.targets.len(),
            backup = %backup,
            "removed imported copies of Target Scheduler's lights"
        );
        state.auto_stacks.touch_database(
            &db_id,
            crate::server::stack_preview::automatic::RefreshReason::Sync,
        );
    }
    Ok(Json(ApiResponse::success(outcome)))
}
