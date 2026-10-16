//! The calibration page's Duplicates filter and its repair: exposures the
//! library records more than once, and merging them.
//!
//! GET lists the twin groups and what a repair would do, with a token for
//! that plan. POST applies it only if the library still plans to the same
//! token, as a sync preview does. Files are read only inside the database's
//! folders, as the browser reads them, and never moved or deleted.

use axum::{extract::State, Json};
use std::sync::Arc;

use crate::calibration::{self, DuplicateReport, FileAccess, RepairError, RepairOutcome};
use crate::server::api::ApiResponse;
use crate::server::calibration_browser::ReadableRoots;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::handlers::{require_database_management_allowed, AppError};
use crate::server::state::AppState;

const STALE: &str = "The calibration library changed since the preview. Preview the repair again.";

/// Plan from the library as it is now. The rows are read under the shared
/// connection; the files are checked after it is free.
async fn plan(ctx: &DatabaseContext) -> Result<DuplicateReport, AppError> {
    let snapshot = {
        let conn = ctx.db();
        let conn = conn.lock().map_err(AppError::db)?;
        calibration::duplicate_snapshot(&conn).map_err(AppError::db)?
    };
    let roots = ReadableRoots::of(ctx);
    let files = FileAccess::resolving(move |path| roots.holds(path).then(|| path.to_path_buf()));
    tokio::task::spawn_blocking(move || calibration::plan_duplicate_repair(&snapshot, &files))
        .await
        .map_err(|error| {
            AppError::InternalError(format!("calibration duplicate check failed: {error}"))
        })
}

/// GET /api/db/{db}/calibrations/duplicates — every exposure recorded more
/// than once, which row each keeps, and the masters a repair rebuilds.
pub async fn get_duplicates(
    ctx: DbContext,
) -> Result<Json<ApiResponse<DuplicateReport>>, AppError> {
    Ok(Json(ApiResponse::success(plan(&ctx).await?)))
}

#[derive(Debug, serde::Deserialize)]
pub struct RepairRequest {
    /// The token of the plan that was previewed.
    pub token: String,
}

/// POST /api/db/{db}/calibrations/duplicates/repair — apply the previewed
/// plan. Refused when the library or its files have changed since.
pub async fn repair_duplicates(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Json(request): Json<RepairRequest>,
) -> Result<Json<ApiResponse<RepairOutcome>>, AppError> {
    require_database_management_allowed(&state)?;
    // No master is being built from the rows while they merge.
    let _permit = state
        .stack_previews
        .acquire_maintenance_permit()
        .await
        .map_err(AppError::Conflict)?;
    let report = plan(&ctx).await?;
    if report.token != request.token {
        return Err(AppError::Conflict(STALE.into()));
    }
    let outcome = {
        let conn = ctx.db();
        let mut conn = conn.lock().map_err(AppError::db)?;
        calibration::apply_duplicate_repair(&mut conn, &report)
    }
    .map_err(|error| match error {
        RepairError::Stale => AppError::Conflict(STALE.into()),
        RepairError::Failed(error) => AppError::db(error),
    })?;
    if outcome.rows_removed > 0 {
        // The sessions a merged row was measured in are measured again
        // from the rows kept: the rigs the plan named.
        let rigs = crate::calibration::flat_groups::job::Rigs::of(
            report.groups.iter().map(|group| group.rig_uuid.clone()),
        );
        crate::server::flat_measure::queue(
            &state,
            &ctx.database_path,
            crate::calibration::flat_groups::job::Scope::flats(rigs),
        );
        // The merged rows were peers the others were judged by, and what
        // each project's lights get may have changed.
        crate::server::calibration_quality::request(&state, &ctx.database_path);
        crate::server::calibration_health::request(&state, &ctx.database_path);
    }
    Ok(Json(ApiResponse::success(outcome)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    #[tokio::test]
    async fn a_repair_needs_management_and_the_previewed_plan() {
        let images = tempfile::tempdir().unwrap();
        let conn = Connection::open_in_memory().unwrap();
        crate::calibration::ensure_schema(&conn).unwrap();
        let name = "2026-10-09_20-01-02_dark_0001.fits";
        for (id, uuid, folder) in [(1, "kept", "library"), (2, "copy", "drop")] {
            let path = images.path().join(folder).join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, b"one dark exposure").unwrap();
            conn.execute(
                "INSERT INTO psf_guard_calibration_frame
                    (id, frame_uuid, rig_uuid, kind, source_path, source_fingerprint,
                     captured_at, file_size, added_at, updated_at)
                 VALUES (?1, ?2, 'rig', 'dark', ?3, 'f', 1000, 17, 0, 0)",
                params![id, uuid, path.to_string_lossy()],
            )
            .unwrap();
        }
        let state = Arc::new(AppState::new_for_test(conn));
        let mut context = (*state.all_databases().remove(0)).clone();
        context.image_dir_paths = vec![images.path().to_path_buf()];
        // A path of its own, for the flat measuring job's queue.
        context.database_path = "/duplicates-repair/catalog.sqlite".into();
        let context = Arc::new(context);
        let ctx = || DbContext(Arc::clone(&context));

        let Json(preview) = get_duplicates(ctx()).await.unwrap();
        let report = preview.data.unwrap();
        assert_eq!((report.groups.len(), report.physical_rows), (1, 1));
        let repair = |token: &str| {
            repair_duplicates(
                State(Arc::clone(&state)),
                ctx(),
                Json(RepairRequest {
                    token: token.into(),
                }),
            )
        };

        assert!(matches!(
            repair(&report.token).await,
            Err(AppError::Forbidden(_))
        ));
        state.set_allow_database_management(true);
        assert!(matches!(
            repair("an older preview").await,
            Err(AppError::Conflict(_))
        ));
        let Json(applied) = repair(&report.token).await.unwrap();
        assert_eq!(applied.data.unwrap().rows_removed, 1);
        // The merged rows' rig, named by the plan, is queued for flats.
        let path = &context.database_path;
        assert_eq!(crate::server::flat_measure::status(path).rigs, ["rig"]);
        crate::server::flat_measure::cancel(path);
        // The plan is spent: the library now plans to nothing.
        assert!(matches!(
            repair(&report.token).await,
            Err(AppError::Conflict(_))
        ));
        let Json(after) = get_duplicates(ctx()).await.unwrap();
        assert!(after.data.unwrap().groups.is_empty());
        assert!(images.path().join("drop").join(name).is_file());
    }
}
