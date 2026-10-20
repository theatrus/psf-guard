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
}
