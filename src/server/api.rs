use crate::sequence_analysis::{ImageQualityResult, ReferenceValues, SequenceSummary};
use crate::server::state::RefreshStatus;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Clone, PartialEq)]
pub enum ApiRefreshStatus {
    #[serde(rename = "ready")]
    Ready,
    #[serde(rename = "loading")]
    Loading,
    #[serde(rename = "refreshing")]
    Refreshing,
}

impl From<RefreshStatus> for ApiRefreshStatus {
    fn from(status: RefreshStatus) -> Self {
        match status {
            RefreshStatus::NotNeeded => ApiRefreshStatus::Ready,
            RefreshStatus::InProgressServeStale => ApiRefreshStatus::Refreshing,
            RefreshStatus::InProgressWait => ApiRefreshStatus::Loading,
            RefreshStatus::NeedsRefresh => ApiRefreshStatus::Loading,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
    pub status: Option<ApiRefreshStatus>,
}

impl<T> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            status: Some(ApiRefreshStatus::Ready),
        }
    }

    pub fn success_with_status(data: T, status: ApiRefreshStatus) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
            status: Some(status),
        }
    }

    pub fn loading() -> Self {
        Self {
            success: true,
            data: None,
            error: None,
            status: Some(ApiRefreshStatus::Loading),
        }
    }

    pub fn error(message: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(message),
            status: None,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ProjectResponse {
    pub id: i32,
    pub profile_id: String,
    pub profile_name: String,
    pub name: String,
    pub display_name: String, // "Profile -> Project" or just "Project"
    pub description: Option<String>,
    pub has_files: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectOverviewResponse {
    pub id: i32,
    pub profile_id: String,
    pub profile_name: String,
    pub name: String,
    pub display_name: String, // "Profile -> Project" or just "Project"
    pub description: Option<String>,
    pub has_files: bool,
    pub target_count: i32,
    pub total_images: i32,
    pub accepted_images: i32,
    pub rejected_images: i32,
    pub pending_images: i32,
    pub total_desired: i32,
    pub files_found: i32,
    pub files_missing: i32,
    pub date_range: DateRange,
    pub filters_used: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct TargetResponse {
    pub id: i32,
    pub name: String,
    pub ra: Option<f64>,
    pub dec: Option<f64>,
    pub active: bool,
    pub image_count: i32,
    pub accepted_count: i32,
    pub rejected_count: i32,
    pub has_files: bool,
}

#[derive(Debug, Serialize)]
pub struct TargetOverviewResponse {
    pub id: i32,
    pub name: String,
    pub ra: Option<f64>,
    pub dec: Option<f64>,
    pub active: bool,
    pub project_id: i32,
    pub project_name: String,
    pub image_count: i32,
    pub accepted_count: i32,
    pub rejected_count: i32,
    pub pending_count: i32,
    pub total_desired: i32,
    pub files_found: i32,
    pub files_missing: i32,
    pub has_files: bool,
    pub date_range: DateRange,
    pub filters_used: Vec<String>,
    pub coordinates_display: Option<String>, // Human-readable RA/Dec
}

#[derive(Debug, Serialize)]
pub struct DateRange {
    pub earliest: Option<i64>,
    pub latest: Option<i64>,
    pub span_days: Option<i32>,
}

#[derive(Debug, Serialize)]
pub struct OverallStatsResponse {
    pub total_projects: i32,
    pub active_projects: i32, // Projects with images
    pub total_targets: i32,
    pub active_targets: i32, // Targets with images
    pub total_images: i32,
    pub accepted_images: i32,
    pub rejected_images: i32,
    pub pending_images: i32,
    pub total_desired: i32,
    pub files_found: i32,
    pub files_missing: i32,
    pub unique_filters: Vec<String>,
    pub date_range: DateRange,
    pub recent_activity: Vec<RecentActivity>,
}

#[derive(Debug, Serialize)]
pub struct RecentActivity {
    pub date: i64,
    pub images_added: i32,
    pub images_graded: i32,
}

#[derive(Debug, Serialize)]
pub struct ImageResponse {
    pub id: i32,
    pub project_id: i32,
    pub project_name: String,
    pub project_display_name: String,
    pub target_id: i32,
    pub target_name: String,
    pub acquired_date: Option<i64>,
    pub filter_name: String,
    pub grading_status: i32,
    pub reject_reason: Option<String>,
    pub metadata: serde_json::Value,
    pub filesystem_path: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ImageQuery {
    pub project_id: Option<i32>,
    pub target_id: Option<i32>,
    pub status: Option<String>,
    pub limit: Option<i32>,
    pub offset: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateGradeRequest {
    pub status: String, // "accepted", "rejected", "pending"
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StarDetectionResponse {
    pub detected_stars: usize,
    pub average_hfr: f64,
    pub average_fwhm: f64,
    pub stars: Vec<StarInfo>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct StarInfo {
    pub x: f64,
    pub y: f64,
    pub hfr: f64,
    pub fwhm: f64,
    pub brightness: f64,
    pub eccentricity: f64,
}

#[derive(Debug, Deserialize)]
pub struct PreviewOptions {
    pub size: Option<String>, // "screen" or "large"
    pub stretch: Option<bool>,
    pub midtone: Option<f64>,
    pub shadow: Option<f64>,
    pub max_stars: Option<u32>, // Max number of stars to annotate
}

#[derive(Debug, Serialize)]
pub struct ServerInfo {
    pub version: String,
    pub cache_directory: String,
    /// Whether `/api/databases` accepts mutating requests and database sync.
    /// Frontend hides those controls when false.
    pub allow_database_management: bool,
}

/// Summary of one configured database, returned by `GET /api/databases`.
#[derive(Debug, Serialize)]
pub struct DatabaseSummary {
    pub id: String,
    pub name: String,
    pub database_path: String,
    pub image_directories: Vec<String>,
}

/// Database-to-database operations exposed by the management UI.
#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SchedulerSyncKind {
    /// Telescope → local: structure, captures, and optional image data.
    Pull,
    /// Local → telescope: planning settings only.
    PushPlanning,
}

/// Body of `POST /api/databases/{db_id}/sync`. `db_id` is the local working
/// database; `peer_db_id` is the telescope scheduler database.
#[derive(Debug, Deserialize)]
pub struct SchedulerSyncRequest {
    pub peer_db_id: String,
    pub kind: SchedulerSyncKind,
    /// Plan and count without changing either database.
    #[serde(default)]
    pub dry_run: bool,
    /// Pull image-data BLOBs. Defaults to true and has no effect on a planning
    /// push.
    #[serde(default)]
    pub with_image_data: Option<bool>,
    /// Optional project-name substring filter.
    #[serde(default)]
    pub project: Option<String>,
}

/// Insert/update counts for one scheduler table.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SchedulerSyncTableCounts {
    pub inserted: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

impl From<&crate::commands::sync::TableCounts> for SchedulerSyncTableCounts {
    fn from(value: &crate::commands::sync::TableCounts) -> Self {
        Self {
            inserted: value.inserted,
            updated: value.updated,
            unchanged: value.unchanged,
            skipped: value.skipped,
        }
    }
}

/// Result of a database-to-database scheduler sync or dry-run preview.
#[derive(Debug, Serialize)]
pub struct SchedulerSyncResponse {
    pub kind: SchedulerSyncKind,
    pub dry_run: bool,
    pub source_db_id: String,
    pub destination_db_id: String,
    pub exposuretemplate: SchedulerSyncTableCounts,
    pub project: SchedulerSyncTableCounts,
    pub ruleweight: SchedulerSyncTableCounts,
    pub target: SchedulerSyncTableCounts,
    pub exposureplan: SchedulerSyncTableCounts,
    /// Present only for a full pull.
    pub acquiredimage: Option<SchedulerSyncTableCounts>,
    /// Present only for a full pull with image-data syncing enabled.
    pub imagedata: Option<SchedulerSyncTableCounts>,
    pub grade_filled: usize,
    pub grade_preserved: usize,
    pub imagedata_bytes: u64,
    pub total_inserted: usize,
    pub total_updated: usize,
}

/// Body of `POST /api/databases`.
#[derive(Debug, Deserialize)]
pub struct AddDatabaseRequest {
    pub name: String,
    pub db_path: String,
    #[serde(default)]
    pub image_dirs: Vec<String>,
    /// Optional user-supplied slug; if omitted, derived from the path.
    #[serde(default)]
    pub slug: Option<String>,
}

/// Body of `POST /api/databases/create` — create a brand-new Target
/// Scheduler database (vendored schema, user_version 23) and start a
/// background import of the given directories.
#[derive(Debug, Deserialize)]
pub struct CreateDatabaseRequest {
    pub name: String,
    /// Directories of FITS files to import; also become the registry entry's
    /// image_dirs.
    pub image_dirs: Vec<String>,
    /// Where to create the .sqlite file. Defaults to
    /// `<registry dir>/databases/<name-slug>.sqlite`.
    #[serde(default)]
    pub db_path: Option<String>,
    /// Optional user-supplied registry slug; derived from the path if absent.
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub time_gap_days: Option<f64>,
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Queue the separate database quality job after import (default false).
    #[serde(default)]
    pub backfill: Option<bool>,
}

/// `POST /api/databases/create` response: the registered database plus the
/// just-started import job's first progress snapshot.
#[derive(Debug, Serialize)]
pub struct CreateDatabaseResponse {
    pub database: DatabaseSummary,
    pub import: crate::server::import_job::ImportJobProgress,
}

/// Body of `POST /api/db/{db_id}/import` — import FITS folders into an
/// existing database as a background job.
#[derive(Debug, Deserialize, Default)]
pub struct ImportRequest {
    /// Directories to scan; defaults to the database's configured image_dirs.
    #[serde(default)]
    pub image_dirs: Option<Vec<String>>,
    #[serde(default)]
    pub time_gap_days: Option<f64>,
    #[serde(default)]
    pub profile_id: Option<String>,
    /// Plan + count without writing (the transaction is rolled back). No
    /// quality job is queued afterwards.
    #[serde(default)]
    pub dry_run: bool,
    /// Queue the separate database quality job after import (default false).
    #[serde(default)]
    pub backfill: Option<bool>,
    /// Attach frames to existing targets (name/coordinate match) instead of
    /// synthesizing new structure for them (default true).
    #[serde(default)]
    pub attach_existing: Option<bool>,
    /// Coordinate-match radius in degrees (default 0.5).
    #[serde(default)]
    pub match_radius_deg: Option<f64>,
}

/// Status returned by both the import-start and import-progress endpoints.
#[derive(Debug, Serialize)]
pub struct ImportStatusResponse {
    /// POST: whether this request started a new job. GET: whether a job is
    /// currently running.
    pub started: bool,
    pub progress: crate::server::import_job::ImportJobProgress,
}

/// Query for `GET /api/db/{db_id}/export` — stream selected lights as a
/// store-mode zip laid out `<target>/LIGHT/<filter>/...` (rejects excluded).
#[derive(Debug, Deserialize, Default)]
pub struct ExportQuery {
    #[serde(default)]
    pub project_id: Option<i32>,
    #[serde(default)]
    pub target_id: Option<i32>,
    #[serde(default)]
    pub include_pending: bool,
    /// Restrict to one filter name (exact, case-insensitive).
    #[serde(default)]
    pub filter_name: Option<String>,
}

/// Body of `POST /api/db/{db_id}/export/local` — place the selected lights
/// into a folder on the SERVER's filesystem (desktop/Tauri mode, where the
/// server is the user's own machine). Management-gated: on a remote server
/// this writes to arbitrary paths.
#[derive(Debug, Deserialize)]
pub struct LocalExportRequest {
    /// Destination folder (absolute path on the server machine).
    pub dest: String,
    #[serde(default)]
    pub project_id: Option<i32>,
    #[serde(default)]
    pub target_id: Option<i32>,
    #[serde(default)]
    pub include_pending: bool,
    #[serde(default)]
    pub filter_name: Option<String>,
    /// Hardlink instead of copy (instant, no extra disk on the same
    /// filesystem; automatically falls back to copy). Default true.
    #[serde(default)]
    pub link: Option<bool>,
    #[serde(default)]
    pub dry_run: bool,
}

/// Body of `PUT /api/db/{db_id}/projects/{project_id}`.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateProjectRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub state: Option<i32>,
    #[serde(default)]
    pub priority: Option<i32>,
    #[serde(default)]
    pub minimum_time: Option<i32>,
    #[serde(default)]
    pub minimum_altitude: Option<f64>,
    #[serde(default)]
    pub maximum_altitude: Option<f64>,
    #[serde(default)]
    pub use_custom_horizon: Option<bool>,
    #[serde(default)]
    pub horizon_offset: Option<f64>,
    #[serde(default)]
    pub meridian_window: Option<i32>,
    #[serde(default)]
    pub filter_switch_frequency: Option<i32>,
    #[serde(default)]
    pub dither_every: Option<i32>,
    #[serde(default)]
    pub enable_grader: Option<bool>,
    #[serde(default)]
    pub is_mosaic: Option<bool>,
}

/// Body of `PUT /api/db/{db_id}/targets/{target_id}` — rename and/or move a
/// target to another project (same profile).
#[derive(Debug, Deserialize, Default)]
pub struct UpdateTargetRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub project_id: Option<i32>,
    #[serde(default)]
    pub active: Option<bool>,
    #[serde(default)]
    pub ra_hours: Option<f64>,
    #[serde(default)]
    pub dec_degrees: Option<f64>,
    #[serde(default)]
    pub epoch_code: Option<i32>,
    #[serde(default)]
    pub rotation: Option<f64>,
    #[serde(default)]
    pub roi: Option<f64>,
}

/// Body of `POST /api/db/{db_id}/projects/{project_id}/merge`.
#[derive(Debug, Deserialize)]
pub struct MergeProjectRequest {
    pub into_project_id: i32,
}

/// Result of a merge: how much moved.
#[derive(Debug, Serialize)]
pub struct MergeProjectResponse {
    pub targets_moved: usize,
    pub images_moved: usize,
}

/// Body of `PUT /api/databases/{db_id}`. All fields are optional; absent fields
/// leave the existing value unchanged.
#[derive(Debug, Deserialize, Default)]
pub struct UpdateDatabaseRequest {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub slug: Option<String>,
    #[serde(default)]
    pub db_path: Option<String>,
    #[serde(default)]
    pub image_dirs: Option<Vec<String>>,
}

#[derive(Debug, Serialize)]
pub struct FileCheckResponse {
    pub images_checked: usize,
    pub files_found: usize,
    pub files_missing: usize,
    pub check_time_ms: u128,
}

#[derive(Debug, Serialize)]
pub struct DirectoryTreeResponse {
    pub total_files: usize,
    pub unique_filenames: usize,
    pub total_directories: usize,
    pub age_seconds: u64,
    pub build_time_ms: u128,
    pub root_directory: String,
}

#[derive(Debug, Serialize)]
pub struct CacheRefreshProgressResponse {
    pub is_refreshing: bool,
    pub stage: String,
    pub progress_percentage: f32,
    pub elapsed_seconds: Option<u64>,
    pub directories_total: usize,
    pub directories_processed: usize,
    pub current_directory_name: Option<String>,
    pub files_scanned: usize,
    pub projects_total: usize,
    pub projects_processed: usize,
    pub current_project_name: Option<String>,
    pub targets_total: usize,
    pub targets_processed: usize,
    pub files_found: usize,
    pub files_missing: usize,
}

#[derive(Debug, Serialize)]
pub struct TargetFilterStats {
    pub filter_name: String,
    pub desired: i32,
    pub acquired: i32,
    pub accepted: i32,
    pub completion_percentage: f64, // accepted / desired * 100
}

#[derive(Debug, Serialize)]
pub struct MissingFileInfo {
    pub filename: String,
    pub image_id: i32,
    pub project_name: String,
    pub target_name: String,
    pub filter_name: Option<String>,
    pub acquired_date: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct FileStatusResponse {
    pub project_id: i32,
    pub project_name: String,
    pub total_images: usize,
    pub files_found: usize,
    pub files_missing: usize,
    pub missing_files: Vec<MissingFileInfo>,
    pub cache_hit_rate: u32,
    pub optimistic_assumption: bool, // true if we assumed files exist due to low hit rate
}

// Sequence analysis request/response types

#[derive(Debug, Deserialize)]
pub struct SequenceAnalysisQuery {
    pub target_id: i32,
    pub filter_name: Option<String>,
    pub session_gap_minutes: Option<u64>,
    pub weight_star_count: Option<f64>,
    pub weight_hfr: Option<f64>,
    pub weight_eccentricity: Option<f64>,
    pub weight_snr: Option<f64>,
    pub weight_background: Option<f64>,
    pub weight_spatial: Option<f64>,
    pub weight_pointing: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct SequenceAnalysisResponse {
    pub sequences: Vec<ScoredSequenceResponse>,
}

/// Request body for starting a spatial (occlusion) metrics scan.
#[derive(Debug, Deserialize)]
pub struct SpatialScanRequest {
    pub target_id: i32,
    pub filter_name: Option<String>,
    /// Recompute all cached quality evidence, including satellite predictions.
    #[serde(default)]
    pub force: bool,
    #[serde(default)]
    pub force_spatial: bool,
    #[serde(default)]
    pub force_astrometry: bool,
    #[serde(default)]
    pub force_satellites: bool,
}

/// Status returned by both the scan-start and scan-progress endpoints.
#[derive(Debug, Serialize)]
pub struct SpatialScanStatusResponse {
    /// POST: whether this request started a new scan. GET: whether a scan is
    /// currently running.
    pub started: bool,
    pub progress: crate::server::spatial_scan::SpatialScanProgress,
    /// Total number of images with cached spatial metrics in this database.
    pub cached_count: usize,
}

#[derive(Debug, Deserialize, Default)]
pub struct QualityBackfillRequest {
    /// Recompute cached star, background, photometry, and pointing evidence.
    #[serde(default)]
    pub force: bool,
}

#[derive(Debug, Serialize)]
pub struct QualityBackfillStatusResponse {
    pub started: bool,
    pub progress: crate::server::quality_backfill::QualityBackfillProgress,
}

#[derive(Debug, Serialize)]
pub struct ScoredSequenceResponse {
    pub target_id: i32,
    pub target_name: String,
    pub filter_name: String,
    pub session_start: Option<i64>,
    pub session_end: Option<i64>,
    pub image_count: usize,
    pub reference_values: ReferenceValues,
    pub images: Vec<ImageQualityResult>,
    pub summary: SequenceSummary,
}

#[derive(Debug, Serialize)]
pub struct ImageQualityContextResponse {
    pub image_id: i32,
    pub quality: Option<ImageQualityResult>,
    pub sequence_target_id: Option<i32>,
    pub sequence_filter_name: Option<String>,
    pub sequence_image_count: Option<usize>,
    pub reference_values: Option<ReferenceValues>,
}
