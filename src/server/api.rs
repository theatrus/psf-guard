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
    pub name: String,
    pub description: Option<String>,
    pub has_files: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectOverviewResponse {
    pub id: i32,
    pub profile_id: String,
    pub name: String,
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
    pub database_path: String,
    pub image_directory: String,
    pub cache_directory: String,
    pub version: String,
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
