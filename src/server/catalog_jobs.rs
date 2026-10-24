//! The catalog jobs of one server: each kind's jobs, one per catalog. They
//! live as long as the server state they work for, so a server started
//! again in the same process, as the desktop app does, starts with none.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::Json;
use serde::{Deserialize, Serialize};

use crate::server::api::ApiResponse;
use crate::server::database_context::DatabaseContext;
use crate::server::extract::DbContext;
use crate::server::job_line::catalog::{CatalogJobs, CatalogWork};
use crate::server::state::AppState;

#[derive(Default)]
pub struct CatalogJobRegistry {
    pub(crate) wbpp_storage: CatalogJobs<crate::server::wbpp_storage::Cleanup>,
    pub(crate) flat_measure: CatalogJobs<crate::server::flat_measure::FlatMeasure>,
    pub(crate) frame_sites: CatalogJobs<crate::server::frame_sites::FrameSites>,
    pub(crate) duplicates: CatalogJobs<crate::server::calibration_duplicates::Duplicates>,
    pub(crate) reject_removal: CatalogJobs<crate::server::reject_removal::RejectRemoval>,
    pub(crate) quality_scans: CatalogJobs<crate::server::quality_scans::QualityScans>,
    pub(crate) calibration_quality:
        CatalogJobs<crate::server::calibration_quality::CalibrationQuality>,
    pub(crate) calibration_health:
        CatalogJobs<crate::server::calibration_health::CalibrationHealth>,
    pub(crate) calibration_scope: CatalogJobs<crate::server::calibration_scope::ScopeWork>,
    pub(crate) flat_group_use: CatalogJobs<crate::server::flat_group_use::FlatGroupUse>,
    pub(crate) arrivals: CatalogJobs<crate::server::quality_arrival::Arrivals>,
    pub(crate) catalog_upkeep: CatalogJobs<crate::server::catalog_upkeep::CatalogUpkeep>,
    pub(crate) backups: CatalogJobs<crate::server::backup_job::Backups>,
    pub(crate) ts_sync: CatalogJobs<crate::server::ts_sync::TsSyncWork>,
    pub(crate) imports: CatalogJobs<crate::server::import_job::Imports>,
    pub(crate) exports: CatalogJobs<crate::server::export_job::Exports>,
    pub(crate) catalog_refresh: CatalogJobs<crate::server::catalog_refresh::CatalogRefresh>,
    pub(crate) pregeneration: CatalogJobs<crate::server::preview_pregeneration::Pregeneration>,
}

/// The automatic jobs a person can pause from the header. A pause holds the
/// job's automatic work in one catalog until Resume or until the server
/// stops; it is not stored. Work a person asked for, such as a quality scan
/// of one target, still runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PausableJob {
    /// The database-wide quality analysis, which also scans new frames.
    Quality,
    Pregeneration,
    Flats,
    Sites,
}

impl PausableJob {
    pub const ALL: [Self; 4] = [Self::Quality, Self::Pregeneration, Self::Flats, Self::Sites];
}

impl CatalogJobRegistry {
    /// Pause `job` for the database `ctx`. Returns false when it was paused
    /// already.
    pub(crate) fn pause(&self, job: PausableJob, ctx: &DatabaseContext) -> bool {
        fn pause<W: CatalogWork>(jobs: &CatalogJobs<W>, catalog: &str) -> bool {
            jobs.job(catalog).pause()
        }
        let catalog = catalog_of(job, ctx);
        match job {
            PausableJob::Quality => pause(&self.quality_scans, &catalog),
            PausableJob::Pregeneration => pause(&self.pregeneration, &catalog),
            PausableJob::Flats => pause(&self.flat_measure, &catalog),
            PausableJob::Sites => pause(&self.frame_sites, &catalog),
        }
    }

    /// End a pause of `job` for the database `ctx`; the work it held starts.
    /// Returns false when the job was not paused.
    pub(crate) fn resume(
        &self,
        state: &Arc<AppState>,
        job: PausableJob,
        ctx: &DatabaseContext,
    ) -> bool {
        let catalog = catalog_of(job, ctx);
        match job {
            PausableJob::Quality => self.quality_scans.resume(state, &catalog),
            PausableJob::Pregeneration => self.pregeneration.resume(state, &catalog),
            PausableJob::Flats => self.flat_measure.resume(state, &catalog),
            PausableJob::Sites => self.frame_sites.resume(state, &catalog),
        }
    }

    /// Whether `job` is paused for the database `ctx`.
    pub(crate) fn paused(&self, job: PausableJob, ctx: &DatabaseContext) -> bool {
        fn paused<W: CatalogWork>(jobs: &CatalogJobs<W>, catalog: &str) -> bool {
            jobs.existing(catalog).is_some_and(|job| job.is_paused())
        }
        let catalog = catalog_of(job, ctx);
        match job {
            PausableJob::Quality => paused(&self.quality_scans, &catalog),
            PausableJob::Pregeneration => paused(&self.pregeneration, &catalog),
            PausableJob::Flats => paused(&self.flat_measure, &catalog),
            PausableJob::Sites => paused(&self.frame_sites, &catalog),
        }
    }

    /// The jobs paused for the database `ctx`.
    pub(crate) fn paused_jobs(&self, ctx: &DatabaseContext) -> Vec<PausableJob> {
        PausableJob::ALL
            .into_iter()
            .filter(|job| self.paused(*job, ctx))
            .collect()
    }
}

/// The key of `ctx`'s catalog in the job's registry.
fn catalog_of(job: PausableJob, ctx: &DatabaseContext) -> String {
    match job {
        PausableJob::Quality => crate::server::quality_scans::scanner(ctx),
        PausableJob::Pregeneration => ctx.id.clone(),
        PausableJob::Flats | PausableJob::Sites => ctx.database_path.clone(),
    }
}

/// `GET /api/db/{db}/jobs/paused`: the jobs a person paused for the
/// database.
pub async fn get_paused(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
) -> Json<ApiResponse<Vec<PausableJob>>> {
    Json(ApiResponse::success(state.catalog_jobs.paused_jobs(&ctx)))
}

/// `POST /api/db/{db}/jobs/{job}/pause`: pause the job for the database
/// until Resume or until the server stops. Returns false when it was
/// paused already.
pub async fn post_pause(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job)): Path<(String, PausableJob)>,
) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(state.catalog_jobs.pause(job, &ctx)))
}

/// `POST /api/db/{db}/jobs/{job}/resume`: end a pause; the work it held
/// starts. Returns false when the job was not paused.
pub async fn post_resume(
    State(state): State<Arc<AppState>>,
    ctx: DbContext,
    Path((_db_id, job)): Path<(String, PausableJob)>,
) -> Json<ApiResponse<bool>> {
    Json(ApiResponse::success(
        state.catalog_jobs.resume(&state, job, &ctx),
    ))
}

#[cfg(test)]
mod tests {
    use crate::server::state::AppState;
    use std::sync::Arc;

    /// A context reaches its server's jobs only once the server started work
    /// for its catalog, and never keeps the server alive.
    #[test]
    fn a_context_links_to_the_server_that_works_for_it() {
        let state = Arc::new(AppState::new_for_test(
            rusqlite::Connection::open_in_memory().unwrap(),
        ));
        let ctx = state.all_databases().remove(0);
        assert!(ctx.server.server().is_none());
        state.link_contexts(&ctx.database_path);
        assert!(Arc::ptr_eq(&ctx.server.server().unwrap(), &state));
        drop(state);
        assert!(ctx.server.server().is_none(), "the link is weak");
    }

    /// Each server state has its own jobs: a second server in the same
    /// process starts with none.
    #[test]
    fn each_server_state_has_its_own_catalog_jobs() {
        let first = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        let second = AppState::new_for_test(rusqlite::Connection::open_in_memory().unwrap());
        first.catalog_jobs.calibration_health.job("catalog.sqlite");
        assert!(first
            .catalog_jobs
            .calibration_health
            .existing("catalog.sqlite")
            .is_some());
        assert!(second
            .catalog_jobs
            .calibration_health
            .existing("catalog.sqlite")
            .is_none());
    }
}
