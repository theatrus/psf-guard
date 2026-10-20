//! The catalog jobs of one server: each kind's jobs, one per catalog. They
//! live as long as the server state they work for, so a server started
//! again in the same process, as the desktop app does, starts with none.

use crate::server::job_line::catalog::CatalogJobs;

#[derive(Default)]
pub struct CatalogJobRegistry {
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
