//! A database's renamed lights in Settings: lights an import catalogued
//! twice because their file was renamed after it was imported, and the
//! merge that folds each twin into the first light (#1020). See
//! [`crate::renamed_lights`].
//!
//! - **Check** (`GET …/renamed-lights`) reads the lights on its own
//!   connection and looks for their files in the directory tree the server
//!   already has. It writes nothing.
//! - **Merge** (`POST …/renamed-lights/merge`) copies the database beside
//!   itself, then plans again and merges in one transaction, as **Fill in
//!   GUIDs** does. Twins a collaboration report names stay.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::{extract::State, Json};
use serde::Serialize;

use crate::commands::reject_removal::RemovedFrame;
use crate::renamed_lights::{self, FileFinder, HeldPair, RenamedPair};
use crate::server::api::ApiResponse;
use crate::server::database_context::{open_scheduler_connection, DatabaseContext};
use crate::server::extract::DbContext;
use crate::server::handlers::AppError;
use crate::server::state::AppState;

/// What the check found.
#[derive(Debug, Serialize)]
pub struct RenamedLightsReport {
    pub pairs: Vec<RenamedPair>,
    pub held: Vec<HeldPair>,
    /// Whether this server can write the database file.
    pub writable: bool,
    /// Image folders that are missing now. A light whose file is in one
    /// would look renamed, so nothing is checked until they are back.
    pub missing_folders: Vec<String>,
}

/// What a merge did, and the copy it took first.
#[derive(Debug, Serialize)]
pub struct RenamedLightsMerged {
    pub merged: Vec<RenamedPair>,
    pub held: Vec<HeldPair>,
    pub backup_path: Option<String>,
}

fn failed(error: impl std::fmt::Display) -> AppError {
    AppError::InternalError(format!("{error:#}"))
}

/// Frames a collaboration report names: none without Director.
async fn protected(state: &AppState) -> Result<BTreeSet<String>, AppError> {
    match state.director.clone() {
        Some(service) => service.reported_image_guids().await.map_err(|error| {
            AppError::InternalError(format!("reading collaboration reports: {error}"))
        }),
        None => Ok(BTreeSet::new()),
    }
}

fn missing_folders(ctx: &DatabaseContext) -> Vec<String> {
    ctx.image_dirs
        .iter()
        .filter(|dir| !std::path::Path::new(dir).is_dir())
        .cloned()
        .collect()
}

/// Look for files in the database's folders. A merge asks for a tree no
/// older than a refresh's interval; a check takes the one there is.
fn finder(ctx: &DatabaseContext, fresh: bool) -> anyhow::Result<FileFinder> {
    let cached = ctx.directory_tree_cache.read().unwrap().clone();
    let tree = match cached {
        Some(tree) if !fresh => tree,
        _ => ctx.refresh_directory_tree_if_needed()?,
    };
    Ok(FileFinder::in_folders(
        Some(tree),
        ctx.image_dir_paths.clone(),
        None,
    ))
}

/// `GET /api/db/{db_id}/renamed-lights` — lights catalogued twice because
/// their file was renamed. Read only.
pub async fn report(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<RenamedLightsReport>>, AppError> {
    let protected = protected(&state).await?;
    let ctx = Arc::clone(&ctx.0);
    let report = tokio::task::spawn_blocking(move || -> anyhow::Result<RenamedLightsReport> {
        let path = std::path::Path::new(&ctx.database_path);
        let writable = crate::ts_guids::is_writable(path);
        let missing_folders = missing_folders(&ctx);
        if !missing_folders.is_empty() {
            return Ok(RenamedLightsReport {
                pairs: Vec::new(),
                held: Vec::new(),
                writable,
                missing_folders,
            });
        }
        let conn = crate::ts_guids::open_existing(path, false)?;
        let report = renamed_lights::plan(&conn, &finder(&ctx, false)?, &protected)?;
        Ok(RenamedLightsReport {
            pairs: report.pairs,
            held: report.held,
            writable,
            missing_folders,
        })
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    Ok(Json(ApiResponse::success(report)))
}

/// `POST /api/db/{db_id}/renamed-lights/merge` — copy the database beside
/// itself, then fold each twin into its first light. Runs off the request
/// path on its own connection.
pub async fn merge(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Result<Json<ApiResponse<RenamedLightsMerged>>, AppError> {
    if let Some(dir) = missing_folders(&ctx).first() {
        return Err(AppError::BadRequest(format!(
            "image folder {dir} is missing; a light whose file is there would look renamed. \
             Connect it, or remove it from the database, first."
        )));
    }
    let protected = protected(&state).await?;
    let ctx = Arc::clone(&ctx.0);
    let merged = tokio::task::spawn_blocking(move || -> anyhow::Result<RenamedLightsMerged> {
        let finder = finder(&ctx, true)?;
        let repaired = {
            let _guard = ctx
                .organization_mutex
                .lock()
                .map_err(|_| anyhow::anyhow!("the catalog's organization lock is poisoned"))?;
            renamed_lights::repair_database(
                std::path::Path::new(&ctx.database_path),
                &finder,
                &protected,
            )?
        };
        let removed: Vec<RemovedFrame> = repaired
            .outcome
            .merged
            .iter()
            .map(|pair| RemovedFrame {
                image_id: pair.twin.id,
                guid: pair.twin.guid.clone(),
                project_id: pair.twin.project_id,
                target_id: pair.twin.target_id,
            })
            .collect();
        if !removed.is_empty() {
            let conn = open_scheduler_connection(&ctx.database_path)?;
            crate::server::reject_removal::after_change(&ctx, &conn, &removed);
        }
        Ok(RenamedLightsMerged {
            merged: repaired.outcome.merged,
            held: repaired.outcome.held,
            backup_path: repaired.backup.map(|backup| backup.display().to_string()),
        })
    })
    .await
    .map_err(failed)?
    .map_err(failed)?;
    if let Some(backup) = &merged.backup_path {
        tracing::info!(
            merged = merged.merged.len(),
            backup = %backup,
            "merged lights catalogued twice under a renamed file"
        );
    }
    Ok(Json(ApiResponse::success(merged)))
}
