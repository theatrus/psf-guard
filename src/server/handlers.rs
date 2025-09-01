use axum::{
    extract::{Path, Query, State},
    http::{
        header::{CACHE_CONTROL, CONTENT_TYPE},
        StatusCode,
    },
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::AsyncReadExt;

use crate::db::Database;
use crate::models::GradingStatus;
use crate::server::api::*;
use crate::server::state::AppState;

pub async fn get_server_info(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<ServerInfo>>, AppError> {
    let info = ServerInfo {
        database_path: state.database_path.clone(),
        image_directory: state.image_dirs.join(", "),
        cache_directory: state.cache_dir.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    Ok(Json(ApiResponse::success(info)))
}

pub async fn refresh_file_cache(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<FileCheckResponse>>, AppError> {
    let start_time = std::time::Instant::now();

    tracing::info!("🔄 Starting file cache refresh (includes directory tree cache)");

    // Refresh projects (this will also refresh the directory tree cache first)
    refresh_project_cache(&state).await?;

    // Get stats
    let (images_checked, files_found, files_missing) = {
        let cache = state.file_check_cache.read().unwrap();
        let total_projects = cache.projects_with_files.len();
        let found = cache.projects_with_files.values().filter(|&&v| v).count();
        let missing = total_projects - found;
        (total_projects, found, missing)
    };

    let response = FileCheckResponse {
        images_checked,
        files_found,
        files_missing,
        check_time_ms: start_time.elapsed().as_millis(),
    };

    tracing::info!(
        "✅ File cache refresh completed in {}ms - {} projects checked, {} with files, {} missing",
        response.check_time_ms,
        images_checked,
        files_found,
        files_missing
    );

    Ok(Json(ApiResponse::success(response)))
}

pub async fn refresh_directory_tree_cache(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<DirectoryTreeResponse>>, AppError> {
    let start_time = std::time::Instant::now();

    tracing::info!("🌳 Starting directory tree cache refresh");

    // Force rebuild the directory tree cache
    let directory_tree = state.rebuild_directory_tree().map_err(|e| {
        tracing::error!("Failed to rebuild directory tree cache: {}", e);
        AppError::InternalError(format!("Cache rebuild failed: {}", e))
    })?;

    let build_time_ms = start_time.elapsed().as_millis();
    let stats = directory_tree.stats();

    let response = DirectoryTreeResponse {
        total_files: stats.total_files,
        unique_filenames: stats.unique_filenames,
        total_directories: stats.total_directories,
        age_seconds: stats.age.as_secs(),
        build_time_ms,
        root_directory: stats
            .roots
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
    };

    tracing::info!(
        "✅ Directory tree cache refresh completed in {}ms - {} files, {} directories",
        build_time_ms,
        stats.total_files,
        stats.total_directories
    );

    Ok(Json(ApiResponse::success(response)))
}

pub async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ProjectResponse>>>, AppError> {
    tracing::debug!("📋 Listing projects");

    // Check if cache is expired or empty
    let (needs_refresh, has_stale_data) = {
        let cache = state.file_check_cache.read().unwrap();
        let expired = cache.is_expired();
        let empty = cache.projects_with_files.is_empty();

        if expired {
            tracing::debug!("⏰ Project cache expired");
        } else if empty {
            tracing::debug!("📭 Project cache empty");
        }

        let needs_refresh = expired || empty;
        let has_stale_data = !empty; // We have stale data if cache is not empty

        (needs_refresh, has_stale_data)
    };

    // If cache is expired but we have stale data, serve stale and refresh in background
    if needs_refresh {
        if has_stale_data {
            // Serve stale cache immediately, refresh in background
            tracing::debug!("🔄 Serving stale cache, refreshing in background");
            state.spawn_background_refresh();
        } else {
            // Cache is empty, we must refresh synchronously
            tracing::debug!("🔄 Cache empty, refreshing synchronously");
            refresh_project_cache(&state).await?;
        }
    }

    // Get file existence info from cache (may be stale, but that's okay)
    let file_existence_map: HashMap<i32, bool> = {
        let cache = state.file_check_cache.read().unwrap();
        cache.projects_with_files.clone()
    };

    // Get ALL projects from database (not just those with files)
    let projects = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        db.get_projects_with_images()
            .map_err(|_| AppError::DatabaseError)?
    };

    let response: Vec<ProjectResponse> = projects
        .into_iter()
        .map(|p| ProjectResponse {
            id: p.id,
            name: p.name,
            description: p.description,
            has_files: file_existence_map.get(&p.id).copied().unwrap_or(false),
        })
        .collect();

    tracing::debug!("📋 Returning {} projects", response.len());

    Ok(Json(ApiResponse::success(response)))
}

pub async fn refresh_project_cache(state: &Arc<AppState>) -> Result<(), AppError> {
    let start_time = std::time::Instant::now();
    tracing::debug!("🔄 Refreshing project cache");

    // First, refresh the directory tree cache to ensure file lookups are up-to-date
    if let Err(e) = state.refresh_directory_tree_if_needed() {
        tracing::warn!(
            "⚠️ Directory tree cache refresh failed during project cache refresh: {}",
            e
        );
        // Continue with project cache refresh even if directory tree refresh fails
    } else {
        tracing::debug!("✅ Directory tree cache ready for project cache refresh");
    }

    // Get all projects - we'll check files using the directory tree cache instead of sampling
    let projects = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        db.get_projects_with_images()
            .map_err(|_| AppError::DatabaseError)?
    };

    let project_count = projects.len();

    tracing::debug!("🔍 Checking {} projects for file existence", project_count);

    // Perform all expensive filesystem operations WITHOUT holding any locks
    let mut cache_updates = HashMap::new();
    let mut projects_with_files = 0;

    for project in &projects {
        tracing::debug!(
            "🔎 Checking project '{}' (ID: {}) using directory tree cache",
            project.name,
            project.id
        );

        let has_files = check_project_files_via_cache(state, project.id).await?;

        tracing::debug!(
            "📊 Project '{}' (ID: {}): has_files = {}",
            project.name,
            project.id,
            has_files
        );

        if has_files {
            projects_with_files += 1;
        }

        cache_updates.insert(project.id, has_files);
    }

    // Log which projects have files for debugging before updating cache
    for (project_id, has_files) in &cache_updates {
        if let Some(project) = projects.iter().find(|p| p.id == *project_id) {
            tracing::debug!(
                "📊 Project '{}' (ID: {}): has_files = {}",
                project.name,
                project.id,
                has_files
            );
        }
    }

    // Only acquire write lock for the final atomic update
    {
        let mut cache = state.file_check_cache.write().unwrap();
        cache.projects_with_files = cache_updates;
        cache.last_updated = std::time::Instant::now();
    }

    let duration = start_time.elapsed();
    tracing::info!(
        "✅ Project cache refresh completed in {:?} - {}/{} projects have files",
        duration,
        projects_with_files,
        project_count
    );

    Ok(())
}

// Efficient function that checks ALL files for a project using the directory tree cache
async fn check_project_files_via_cache(
    state: &Arc<AppState>,
    project_id: i32,
) -> Result<bool, AppError> {
    // Get the directory tree cache
    let directory_tree = state.get_directory_tree().map_err(|e| {
        tracing::error!("Failed to get directory tree cache: {}", e);
        AppError::InternalError("Directory cache error".to_string())
    })?;

    // Get all images for this project from database
    let all_images = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        db.get_images_by_project_id(project_id)
            .map_err(|_| AppError::DatabaseError)?
    };

    if all_images.is_empty() {
        tracing::debug!("  📭 Project {} has no images in database", project_id);
        return Ok(false);
    }

    tracing::debug!(
        "  🔍 Checking {} images for project {}",
        all_images.len(),
        project_id
    );

    let mut found_files = 0;
    let mut total_valid_files = 0;
    let mut cache_hits = 0;

    // Check each image using the directory tree cache
    for (image, _project_name, _target_name) in &all_images {
        if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
            if let Some(filename_path) = metadata["FileName"].as_str() {
                // Extract just the filename from the full path
                let filename = filename_path
                    .split(&['\\', '/'][..])
                    .next_back()
                    .unwrap_or(filename_path);

                total_valid_files += 1;

                // Check if this filename exists in the directory tree cache
                if directory_tree.find_file_first(filename).is_some() {
                    cache_hits += 1;
                    found_files += 1;

                    tracing::trace!("    ✅ Cache hit for: {}", filename);

                    // Early exit optimization: if we found any files, we know the project has files
                    if found_files >= 5 {
                        tracing::debug!(
                            "  🚀 Early exit: Found {} files ({}% cache hit rate), project has files",
                            found_files,
                            (cache_hits * 100) / total_valid_files
                        );
                        return Ok(true);
                    }
                } else {
                    tracing::trace!("    ❌ Cache miss for: {}", filename);
                }
            }
        }
    }

    let has_files = found_files > 0;
    let hit_rate = if total_valid_files > 0 {
        (cache_hits * 100) / total_valid_files
    } else {
        0
    };

    tracing::debug!(
        "  📊 Project {} check result: {}/{} files found ({}% cache hit rate), has_files = {}",
        project_id,
        found_files,
        total_valid_files,
        hit_rate,
        has_files
    );

    Ok(has_files)
}

pub async fn list_targets(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<Json<ApiResponse<Vec<TargetResponse>>>, AppError> {
    tracing::debug!("🎯 Listing targets for project {}", project_id);

    // Check if cache is expired or empty
    let (needs_refresh, has_stale_data) = {
        let cache = state.file_check_cache.read().unwrap();
        let expired = cache.is_expired();
        let empty = cache.targets_with_files.is_empty();

        if expired {
            tracing::debug!("⏰ Target cache expired for project {}", project_id);
        } else if empty {
            tracing::debug!("📭 Target cache empty for project {}", project_id);
        }

        let needs_refresh = expired || empty;
        let has_stale_data = !empty; // We have stale data if cache is not empty

        (needs_refresh, has_stale_data)
    };

    // If cache is expired but we have stale data, serve stale and refresh in background
    if needs_refresh {
        if has_stale_data {
            // Serve stale cache immediately, refresh in background
            tracing::debug!("🔄 Serving stale target cache, refreshing in background");
            state.spawn_background_refresh();
        } else {
            // Cache is empty, we must refresh synchronously
            tracing::debug!("🔄 Target cache empty, refreshing synchronously");
            refresh_target_cache(&state, project_id).await?;
        }
    }

    // Get file existence info from cache (may be stale, but that's okay)
    let file_existence_map: HashMap<i32, bool> = {
        let cache = state.file_check_cache.read().unwrap();
        cache.targets_with_files.clone()
    };

    // Get ALL targets from database (not just those with files)
    let targets = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        db.get_targets_with_images(project_id)
            .map_err(|_| AppError::DatabaseError)?
    };

    let response: Vec<TargetResponse> = targets
        .into_iter()
        .map(|(target, img_count, accepted, rejected)| TargetResponse {
            id: target.id,
            name: target.name,
            ra: target.ra,
            dec: target.dec,
            active: target.active,
            image_count: img_count,
            accepted_count: accepted,
            rejected_count: rejected,
            has_files: file_existence_map.get(&target.id).copied().unwrap_or(false),
        })
        .collect();

    tracing::debug!(
        "🎯 Returning {} targets for project {}",
        response.len(),
        project_id
    );

    Ok(Json(ApiResponse::success(response)))
}

async fn refresh_target_cache(state: &Arc<AppState>, project_id: i32) -> Result<(), AppError> {
    let start_time = std::time::Instant::now();
    tracing::debug!("🔄 Refreshing target cache for project {}", project_id);

    // Ensure directory tree cache is available before file lookups
    if let Err(e) = state.refresh_directory_tree_if_needed() {
        tracing::warn!(
            "⚠️ Directory tree cache refresh failed during target cache refresh: {}",
            e
        );
        // Continue with target cache refresh even if directory tree refresh fails
    } else {
        tracing::debug!("✅ Directory tree cache ready for target cache refresh");
    }

    let targets_to_check = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);
        db.get_targets_with_images(project_id)
            .map_err(|_| AppError::DatabaseError)?
    };

    let target_count = targets_to_check.len();
    tracing::debug!(
        "🔍 Checking {} targets in project {}",
        target_count,
        project_id
    );

    // Perform all expensive filesystem operations WITHOUT holding any locks
    let mut cache_updates = HashMap::new();
    let mut targets_with_files = 0;

    for (target, _, _, _) in targets_to_check {
        tracing::trace!("🔎 Checking target '{}' (ID: {})", target.name, target.id);
        let has_files = check_target_has_files(state, target.id).await?;

        if has_files {
            targets_with_files += 1;
        }

        cache_updates.insert(target.id, has_files);
    }

    // Only acquire write lock for the final atomic update
    {
        let mut cache = state.file_check_cache.write().unwrap();
        // Merge updates into existing cache
        for (tid, has_files) in cache_updates {
            cache.targets_with_files.insert(tid, has_files);
        }
        cache.last_updated = std::time::Instant::now();
    }

    let duration = start_time.elapsed();
    tracing::debug!(
        "✅ Target cache refresh completed for project {} in {:?} - {}/{} targets have files",
        project_id,
        duration,
        targets_with_files,
        target_count
    );

    Ok(())
}

async fn check_target_has_files(state: &Arc<AppState>, target_id: i32) -> Result<bool, AppError> {
    let images_to_check = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        db.query_images(None, None, None, None)
            .map_err(|_| AppError::DatabaseError)?
            .into_iter()
            .filter(|(img, _, _)| img.target_id == target_id)
            .take(3) // Check up to 3 images
            .collect::<Vec<_>>()
    };

    for (image, _, target_name) in images_to_check {
        if let Ok(metadata) = serde_json::from_str::<serde_json::Value>(&image.metadata) {
            if let Some(filename) = metadata["FileName"].as_str() {
                let file_only = filename
                    .split(&['\\', '/'][..])
                    .next_back()
                    .unwrap_or(filename);
                if find_fits_file(state, &image, &target_name, file_only).is_ok() {
                    return Ok(true);
                }
            }
        }
    }

    Ok(false)
}

pub async fn get_images(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ImageQuery>,
) -> Result<Json<ApiResponse<Vec<ImageResponse>>>, AppError> {
    let conn = state.db();
    let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
    let db = Database::new(&conn);

    // Convert status string to GradingStatus enum
    let status_filter = params.status.as_ref().and_then(|s| match s.as_str() {
        "pending" => Some(GradingStatus::Pending),
        "accepted" => Some(GradingStatus::Accepted),
        "rejected" => Some(GradingStatus::Rejected),
        _ => None,
    });

    let images = db
        .query_images(status_filter, None, None, None)
        .map_err(|_| AppError::DatabaseError)?;

    // Filter by project_id and target_id if provided
    let filtered_images: Vec<_> = images
        .into_iter()
        .filter(|(img, _, _)| {
            params.project_id.is_none_or(|id| img.project_id == id)
                && params.target_id.is_none_or(|id| img.target_id == id)
        })
        .collect();

    // Apply limit and offset
    let offset = params.offset.unwrap_or(0) as usize;
    let limit = params.limit.unwrap_or(100) as usize;

    let response: Vec<ImageResponse> = filtered_images
        .into_iter()
        .skip(offset)
        .take(limit)
        .map(|(img, proj_name, target_name)| {
            let metadata: serde_json::Value = serde_json::from_str(&img.metadata)
                .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

            ImageResponse {
                id: img.id,
                project_id: img.project_id,
                project_name: proj_name,
                target_id: img.target_id,
                target_name,
                acquired_date: img.acquired_date,
                filter_name: img.filter_name,
                grading_status: img.grading_status,
                reject_reason: img.reject_reason,
                metadata,
                filesystem_path: None, // Not calculated for bulk operations for performance
            }
        })
        .collect();

    Ok(Json(ApiResponse::success(response)))
}

#[axum::debug_handler]
pub async fn get_image(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
) -> Result<Json<ApiResponse<ImageResponse>>, AppError> {
    use crate::image_analysis::FitsImage;

    // Get image data from database first (before any async operations)
    let (image, proj_name, target_name, mut metadata) = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| AppError::DatabaseError)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get project and target names
        let all_images = db
            .query_images(None, None, None, None)
            .map_err(|_| AppError::DatabaseError)?;

        let (_, proj_name, target_name) = all_images
            .into_iter()
            .find(|(img, _, _)| img.id == image_id)
            .ok_or(AppError::NotFound)?;

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .unwrap_or(serde_json::Value::Object(serde_json::Map::new()));

        (image, proj_name, target_name, metadata)
    }; // Database connection is dropped here

    // Now we can do async operations
    let stats_cache_filename = format!(
        "stats_{}_{}_{}_{}.json",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0)
    );
    let stats_cache_path = state.get_cache_path("stats", &stats_cache_filename);

    // Ensure cache directory exists
    if let Some(parent) = stats_cache_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    // Try to resolve the filesystem path for the FITS file
    let filesystem_path = metadata["FileName"].as_str().and_then(|filename| {
        filename
            .split(&['\\', '/'][..])
            .next_back()
            .map(|file_only| find_fits_file(&state, &image, &target_name, file_only))
    });

    let resolved_fits_path = filesystem_path
        .as_ref()
        .and_then(|result| result.as_ref().ok());
    let filesystem_path_string = resolved_fits_path.map(|p| p.to_string_lossy().to_string());

    // Check if statistics are already cached
    let fits_stats = if tokio::fs::metadata(&stats_cache_path).await.is_ok() {
        // Load from cache
        match tokio::fs::read_to_string(&stats_cache_path).await {
            Ok(cached_data) => serde_json::from_str::<serde_json::Value>(&cached_data).ok(),
            Err(_) => None,
        }
    } else if let Some(fits_path) = resolved_fits_path {
        // Calculate statistics from FITS file
        if let Ok(fits) = FitsImage::from_file(fits_path) {
            let stats = fits.calculate_basic_statistics();

            // Extract temperature and camera model from FITS headers
            let temperature = FitsImage::extract_temperature(fits_path);
            let camera_model = FitsImage::extract_camera_model(fits_path);

            let mut stats_json = serde_json::json!({
                "Min": stats.min,
                "Max": stats.max,
                "Mean": stats.mean,
                "Median": stats.median,
                "StdDev": stats.std_dev,
                "Mad": stats.mad
            });

            // Add temperature if available
            if let Some(temp) = temperature {
                stats_json["Temperature"] = serde_json::json!(temp);
            }

            // Add camera model if available
            if let Some(camera) = camera_model {
                stats_json["Camera"] = serde_json::json!(camera);
            }

            // Cache the statistics
            if let Ok(cached_data) = serde_json::to_string(&stats_json) {
                let _ = tokio::fs::write(&stats_cache_path, cached_data).await;
            }

            Some(stats_json)
        } else {
            None
        }
    } else {
        None
    };

    // Merge statistics into metadata if available
    if let (Some(stats), Some(metadata_obj)) = (fits_stats, metadata.as_object_mut()) {
        if let Some(stats_obj) = stats.as_object() {
            for (key, value) in stats_obj {
                metadata_obj.insert(key.clone(), value.clone());
            }
        }
    }

    let response = ImageResponse {
        id: image.id,
        project_id: image.project_id,
        project_name: proj_name,
        target_id: image.target_id,
        target_name,
        acquired_date: image.acquired_date,
        filter_name: image.filter_name,
        grading_status: image.grading_status,
        reject_reason: image.reject_reason,
        metadata,
        filesystem_path: filesystem_path_string,
    };

    Ok(Json(ApiResponse::success(response)))
}

pub async fn update_image_grade(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
    Json(request): Json<UpdateGradeRequest>,
) -> Result<Json<ApiResponse<()>>, AppError> {
    let conn = state.db();
    let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
    let db = Database::new(&conn);

    let status = match request.status.as_str() {
        "pending" => GradingStatus::Pending,
        "accepted" => GradingStatus::Accepted,
        "rejected" => GradingStatus::Rejected,
        _ => return Err(AppError::BadRequest("Invalid status".to_string())),
    };

    db.update_grading_status(image_id, status, request.reason.as_deref())
        .map_err(|_| AppError::DatabaseError)?;

    Ok(Json(ApiResponse::success(())))
}

// Image preview endpoint
#[axum::debug_handler]
pub async fn get_image_preview(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
    Query(options): Query<PreviewOptions>,
) -> Result<impl IntoResponse, AppError> {
    let start_time = std::time::Instant::now();
    let size = options.size.as_deref().unwrap_or("screen");
    tracing::debug!(
        "🖼️  Generating preview for image {} (size: {})",
        image_id,
        size
    );

    use crate::image_analysis::FitsImage;
    use crate::server::cache::CacheManager;

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        // Get image metadata
        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| AppError::DatabaseError)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(|_| AppError::DatabaseError)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        // Extract filename from metadata
        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        // Extract just the filename from the full path
        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    }; // Connection is dropped here

    // Determine cache parameters
    let stretch = options.stretch.unwrap_or(true);
    let midtone = options.midtone.unwrap_or(0.2);
    let shadow = options.shadow.unwrap_or(-2.8);

    // Create comprehensive cache key including file identity and acquisition details
    let cache_key = format!(
        "{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0), // Include acquisition timestamp
        file_only.replace(&['.', ' ', '-'][..], "_"), // Include filename
        size,
        if stretch { "stretch" } else { "linear" },
        (midtone * 10000.0) as i32, // Higher precision
        (shadow * 10000.0) as i32   // Higher precision
    );

    tracing::trace!("🔑 Cache key for image {}: {}", image_id, cache_key);

    let cache_manager = CacheManager::new(PathBuf::from(&state.cache_dir));
    if let Err(e) = cache_manager.ensure_category_dir("previews") {
        tracing::error!(
            "❌ Failed to create cache directory for image {}: {}",
            image_id,
            e
        );
        return Err(AppError::InternalError(format!(
            "Failed to create cache directory: {}",
            e
        )));
    }
    let cache_path = cache_manager.get_cached_path("previews", &cache_key, "png");

    tracing::trace!("📂 Cache path for image {}: {:?}", image_id, cache_path);

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        tracing::debug!("💾 Cache HIT for image {} - serving from cache", image_id);

        // Serve from cache
        let mut file = File::open(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .await
            .map_err(|_| AppError::InternalError("Failed to read file".to_string()))?;

        tracing::debug!(
            "⚡ Preview served from cache for image {} in {:?}",
            image_id,
            start_time.elapsed()
        );

        return Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/png"),
                (CACHE_CONTROL, "max-age=86400"), // TODO: Use configurable cache expiry
            ],
            buffer,
        ));
    }

    tracing::debug!("💫 Cache MISS for image {} - generating preview", image_id);

    // Add timeout for file finding to prevent hanging
    let find_start = std::time::Instant::now();

    // Find the FITS file
    let fits_path = match find_fits_file(&state, &image, &target_name, &file_only) {
        Ok(path) => {
            tracing::info!(
                "✅ Found FITS file for image {} in {:?}: {:?}",
                image_id,
                find_start.elapsed(),
                path
            );
            path
        }
        Err(AppError::NotFound) => {
            tracing::warn!(
                "⚠️ FITS file not found for image {} (filename: {}) after {:?}",
                image_id,
                file_only,
                find_start.elapsed()
            );
            return Err(AppError::NotFound);
        }
        Err(e) => {
            tracing::error!(
                "❌ Error finding FITS file for image {} (filename: {}) after {:?}: {:?}",
                image_id,
                file_only,
                find_start.elapsed(),
                e
            );
            return Err(e);
        }
    };

    // Load FITS file (just to verify it exists and is valid)
    let _fits = match FitsImage::from_file(&fits_path) {
        Ok(fits) => fits,
        Err(e) => {
            tracing::error!(
                "❌ Failed to load FITS file for image {} at path {:?}: {}",
                image_id,
                fits_path,
                e
            );
            return Err(AppError::InternalError(format!(
                "Failed to load FITS: {}",
                e
            )));
        }
    };

    // Determine target size
    let max_dimensions = match size {
        "large" => Some((2000, 2000)),
        "screen" => Some((1200, 1200)),
        "original" => None,      // No resize for original
        _ => Some((1200, 1200)), // Default to screen size for unknown values
    };

    // Use the existing stretch_to_png function to write directly to cache
    use crate::commands::stretch_to_png::stretch_to_png_with_resize;

    // Create a temporary path for the cache file
    let cache_path_str = cache_path.to_string_lossy().to_string();

    // Generate the stretched PNG with optional resizing
    tracing::trace!(
        "🎨 Generating PNG for image {} with size {:?}",
        image_id,
        max_dimensions
    );

    // Generate the PNG - wrap in spawn_blocking to prevent blocking the async runtime
    tracing::trace!(
        "🎨 Starting PNG generation for image {} to cache path: {}",
        image_id,
        cache_path_str
    );

    let fits_path_str = fits_path.to_string_lossy().to_string();
    let cache_path_str_clone = cache_path_str.clone();
    let generation_result = tokio::task::spawn_blocking(move || {
        stretch_to_png_with_resize(
            &fits_path_str,
            Some(cache_path_str_clone),
            midtone,
            shadow,
            false, // logarithmic
            false, // invert
            max_dimensions,
        )
    })
    .await
    .map_err(|e| {
        tracing::error!(
            "❌ PNG generation task panicked for image {}: {}",
            image_id,
            e
        );
        AppError::InternalError("PNG generation task failed".to_string())
    })?;

    match generation_result {
        Ok(_) => {
            tracing::trace!("✅ PNG generation completed for image {}", image_id);
        }
        Err(e) => {
            tracing::error!(
                "❌ Failed to generate PNG preview for image {} from {:?}: {}",
                image_id,
                fits_path,
                e
            );
            return Err(AppError::InternalError(format!(
                "Failed to generate preview: {}",
                e
            )));
        }
    }

    // Read the file back into memory
    let png_buffer = match tokio::fs::read(&cache_path).await {
        Ok(buffer) => {
            tracing::trace!(
                "📖 Read generated PNG for image {} ({} bytes)",
                image_id,
                buffer.len()
            );
            buffer
        }
        Err(e) => {
            tracing::error!(
                "❌ Failed to read generated PNG for image {} from cache path {}: {}",
                image_id,
                cache_path_str,
                e
            );
            // Try to clean up the potentially corrupted cache file
            let _ = tokio::fs::remove_file(&cache_path).await;
            return Err(AppError::InternalError(
                "Failed to read generated PNG".to_string(),
            ));
        }
    };

    tracing::debug!(
        "✅ Generated and cached preview for image {} in {:?} ({} bytes)",
        image_id,
        start_time.elapsed(),
        png_buffer.len()
    );

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
        ],
        png_buffer,
    ))
}

// Helper function to find FITS file
pub fn find_fits_file(
    state: &AppState,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    use crate::commands::filter_rejected::get_possible_paths;

    tracing::debug!(
        "🔍 find_fits_file called for image_id={}, filename={}, target={}, base_dirs={:?}",
        image.id,
        filename,
        target_name,
        state.image_dirs
    );

    // Extract date from acquired_date
    let acquired_date = image
        .acquired_date
        .and_then(|d| chrono::DateTime::from_timestamp(d, 0))
        .ok_or_else(|| {
            tracing::error!(
                "❌ Invalid date for image {}: {:?}",
                image.id,
                image.acquired_date
            );
            AppError::BadRequest("Invalid date".to_string())
        })?;

    let date_str = acquired_date.format("%Y-%m-%d").to_string();
    tracing::debug!("📅 Date string for image {}: {}", image.id, date_str);

    // Try to find the file in different possible locations across all directories
    let mut all_possible_paths = Vec::new();
    for base_dir in &state.image_dirs {
        let paths = get_possible_paths(base_dir, &date_str, target_name, filename);
        all_possible_paths.extend(paths);
    }

    tracing::debug!(
        "🔎 Checking {} possible paths for image {} across {} directories",
        all_possible_paths.len(),
        image.id,
        state.image_dirs.len()
    );

    // Verify all base directories exist (they were checked during startup)
    for base_dir in &state.image_dirs {
        tracing::debug!("✅ Base directory exists: {}", base_dir);
    }

    for (idx, path) in all_possible_paths.iter().enumerate() {
        tracing::debug!(
            "  📁 Path {}: {:?} (exists: {})",
            idx + 1,
            path,
            path.exists()
        );
        if path.exists() {
            tracing::info!("✅ Found file at path {}: {:?}", idx + 1, path);
            return Ok(path.clone());
        }
    }

    tracing::debug!(
        "❌ File not found in standard paths for image {}, trying directory tree cache lookup",
        image.id
    );

    // Try directory tree cache lookup as fallback
    let search_start = std::time::Instant::now();
    let directory_tree = state.get_directory_tree().map_err(|e| {
        tracing::error!("Failed to get directory tree cache: {}", e);
        AppError::InternalError("Directory cache error".to_string())
    })?;

    tracing::debug!(
        "🌳 Directory tree cache has {} total files, {} unique filenames",
        directory_tree.stats().total_files,
        directory_tree.stats().unique_filenames
    );

    if let Some(first_path) = directory_tree.find_file_first(filename) {
        tracing::debug!(
            "🔍 Found first match in directory tree cache for {}",
            filename
        );

        if first_path.exists() {
            tracing::info!(
                "✅ Found file via directory tree cache in {:?}: {:?}",
                search_start.elapsed(),
                first_path
            );
            return Ok(first_path.clone());
        } else {
            tracing::warn!(
                "❌ First cached path is stale for {}: {:?}",
                filename,
                first_path
            );
        }
    } else {
        tracing::debug!(
            "🔍 No matches in directory tree cache for filename: {}",
            filename
        );
    }

    tracing::warn!(
        "❌ File not found in directory tree cache after {:?} for image {} ({})",
        search_start.elapsed(),
        image.id,
        filename
    );
    Err(AppError::NotFound)
}

#[axum::debug_handler]
pub async fn get_image_stars(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
) -> Result<Json<ApiResponse<StarDetectionResponse>>, AppError> {
    use crate::hocus_focus_star_detection::{detect_stars_hocus_focus, HocusFocusParams};
    use crate::image_analysis::FitsImage;
    use crate::psf_fitting::PSFType;
    use crate::server::cache::CacheManager;

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| AppError::DatabaseError)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(|_| AppError::DatabaseError)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    // Create comprehensive cache key for star detection results
    let cache_key = format!(
        "stars_{}_{}_{}_{}_{}",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_")
    );
    let cache_manager = CacheManager::new(PathBuf::from(&state.cache_dir));
    cache_manager
        .ensure_category_dir("stars")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("stars", &cache_key, "json");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        // Read from cache
        let cached_data = tokio::fs::read_to_string(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let response: StarDetectionResponse = serde_json::from_str(&cached_data)
            .map_err(|_| AppError::InternalError("Invalid cached data".to_string()))?;

        return Ok(Json(ApiResponse::success(response)));
    }

    // Find FITS file path first (this is fast)
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;

    // Move expensive operations to spawn_blocking
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let (stars, detected_count, average_hfr, average_fwhm) =
        tokio::task::spawn_blocking(move || {
            // Load FITS file
            let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))?;

            // Run star detection
            let params = HocusFocusParams {
                psf_type: PSFType::Moffat4,
                ..Default::default()
            };

            let detection_result =
                detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);

            // Convert to API response format
            let stars: Vec<StarInfo> = detection_result
                .stars
                .iter()
                .map(|star| {
                    let eccentricity = if let Some(psf) = &star.psf_model {
                        psf.eccentricity
                    } else {
                        0.0
                    };

                    StarInfo {
                        x: star.position.0,
                        y: star.position.1,
                        hfr: star.hfr,
                        fwhm: star.fwhm,
                        brightness: star.brightness,
                        eccentricity,
                    }
                })
                .collect();

            Ok::<(Vec<StarInfo>, usize, f64, f64), anyhow::Error>((
                stars,
                detection_result.stars.len(),
                detection_result.average_hfr,
                detection_result.average_fwhm,
            ))
        })
        .await
        .map_err(|e| AppError::InternalError(format!("Star detection task panicked: {}", e)))?
        .map_err(|e| AppError::InternalError(format!("Failed to detect stars: {}", e)))?;

    let response = StarDetectionResponse {
        detected_stars: detected_count,
        average_hfr,
        average_fwhm,
        stars,
    };

    // Save to cache
    let cached_data = serde_json::to_string(&response)
        .map_err(|_| AppError::InternalError("Failed to serialize response".to_string()))?;

    tokio::fs::write(&cache_path, cached_data)
        .await
        .map_err(|_| AppError::InternalError("Failed to write cache".to_string()))?;

    Ok(Json(ApiResponse::success(response)))
}

#[axum::debug_handler]
pub async fn get_annotated_image(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
    Query(options): Query<PreviewOptions>,
) -> Result<impl IntoResponse, AppError> {
    use crate::commands::annotate_stars_common::create_annotated_image;
    use crate::image_analysis::FitsImage;
    use crate::server::cache::CacheManager;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ColorType, ImageEncoder, Rgb};

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| AppError::DatabaseError)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(|_| AppError::DatabaseError)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    // Determine size parameter
    let size = options.size.as_deref().unwrap_or("screen");

    // Create comprehensive cache key for annotated image
    let max_stars = options.max_stars.unwrap_or(1000);
    let cache_key = format!(
        "annotated_{}_{}_{}_{}_{}_{}_{}",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        size,
        max_stars
    );
    let cache_manager = CacheManager::new(PathBuf::from(&state.cache_dir));
    cache_manager
        .ensure_category_dir("annotated")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("annotated", &cache_key, "png");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        // Serve from cache
        let mut file = File::open(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .await
            .map_err(|_| AppError::InternalError("Failed to read file".to_string()))?;

        return Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/png"),
                (CACHE_CONTROL, "max-age=86400"), // TODO: Use configurable cache expiry
            ],
            buffer,
        ));
    }

    // Find FITS file path first (this is fast)
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;

    // Move expensive operations to spawn_blocking
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let size_str = size.to_string();
    let final_image = tokio::task::spawn_blocking(move || {
        // Load FITS file
        let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))
            .map_err(|e| anyhow::anyhow!("Failed to load FITS: {}", e))?;

        // Create annotated image using the common function
        let max_stars = options.max_stars.unwrap_or(1000) as usize;
        let rgb_image = create_annotated_image(
            &fits,
            max_stars,          // max_stars from parameter
            0.2,                // midtone_factor
            -2.8,               // shadow_clipping
            Rgb([255, 255, 0]), // yellow color
        )
        .map_err(|e| anyhow::anyhow!("Failed to create annotated image: {}", e))?;

        // Resize if needed based on size parameter
        let final_image = match size_str.as_str() {
            "large" => {
                // Check if we need to resize for "large"
                if fits.width > 2000 || fits.height > 2000 {
                    let aspect_ratio = fits.width as f32 / fits.height as f32;
                    let (new_width, new_height) = if fits.width > fits.height {
                        (2000, (2000.0 / aspect_ratio) as u32)
                    } else {
                        ((2000.0 * aspect_ratio) as u32, 2000)
                    };
                    image::imageops::resize(
                        &rgb_image,
                        new_width,
                        new_height,
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    rgb_image
                }
            }
            "screen" => {
                // Resize for screen viewing
                if fits.width > 1200 || fits.height > 1200 {
                    let aspect_ratio = fits.width as f32 / fits.height as f32;
                    let (new_width, new_height) = if fits.width > fits.height {
                        (1200, (1200.0 / aspect_ratio) as u32)
                    } else {
                        ((1200.0 * aspect_ratio) as u32, 1200)
                    };
                    image::imageops::resize(
                        &rgb_image,
                        new_width,
                        new_height,
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    rgb_image
                }
            }
            "original" => rgb_image, // No resize for original
            _ => {
                // Default to screen size for unknown values
                if fits.width > 1200 || fits.height > 1200 {
                    let aspect_ratio = fits.width as f32 / fits.height as f32;
                    let (new_width, new_height) = if fits.width > fits.height {
                        (1200, (1200.0 / aspect_ratio) as u32)
                    } else {
                        ((1200.0 * aspect_ratio) as u32, 1200)
                    };
                    image::imageops::resize(
                        &rgb_image,
                        new_width,
                        new_height,
                        image::imageops::FilterType::Lanczos3,
                    )
                } else {
                    rgb_image
                }
            }
        };

        Ok::<image::RgbImage, anyhow::Error>(final_image)
    })
    .await
    .map_err(|e| {
        AppError::InternalError(format!("Annotated image generation task panicked: {}", e))
    })?
    .map_err(|e| AppError::InternalError(format!("Failed to generate annotated image: {}", e)))?;

    // Save to cache
    let cache_file = std::fs::File::create(&cache_path)
        .map_err(|_| AppError::InternalError("Failed to create cache file".to_string()))?;
    let writer = std::io::BufWriter::new(cache_file);

    // Create PNG encoder with best compression
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Best, FilterType::Adaptive);

    let (img_width, img_height) = final_image.dimensions();

    // Write the image data
    encoder
        .write_image(&final_image, img_width, img_height, ColorType::Rgb8.into())
        .map_err(|_| AppError::InternalError("Failed to write PNG".to_string()))?;

    // Read the file back into memory
    let png_buffer = tokio::fs::read(&cache_path)
        .await
        .map_err(|_| AppError::InternalError("Failed to read generated PNG".to_string()))?;

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
        ],
        png_buffer,
    ))
}

// PSF multi image parameters
#[derive(Deserialize)]
pub struct PsfMultiOptions {
    pub num_stars: Option<usize>,
    pub psf_type: Option<String>,
    pub sort_by: Option<String>,
    pub grid_cols: Option<usize>,
    pub selection: Option<String>,
}

#[axum::debug_handler]
pub async fn get_psf_visualization(
    State(state): State<Arc<AppState>>,
    Path(image_id): Path<i32>,
    Query(options): Query<PsfMultiOptions>,
) -> Result<impl IntoResponse, AppError> {
    use crate::commands::visualize_psf_multi_common::create_psf_multi_image;
    use crate::image_analysis::FitsImage;
    use crate::psf_fitting::PSFType;
    use crate::server::cache::CacheManager;
    use image::codecs::png::{CompressionType, FilterType, PngEncoder};
    use image::{ColorType, ImageEncoder};

    // Get image metadata from database
    let (image, file_only, target_name) = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        let images = db
            .get_images_by_ids(&[image_id])
            .map_err(|_| AppError::DatabaseError)?;

        let image = images.into_iter().next().ok_or(AppError::NotFound)?;

        // Get target name
        let targets = db
            .get_targets_by_ids(&[image.target_id])
            .map_err(|_| AppError::DatabaseError)?;

        let target = targets.into_iter().next().ok_or(AppError::NotFound)?;
        let target_name = target.name.clone();

        let metadata: serde_json::Value = serde_json::from_str(&image.metadata)
            .map_err(|_| AppError::BadRequest("Invalid metadata".to_string()))?;

        let filename = metadata["FileName"]
            .as_str()
            .ok_or_else(|| AppError::BadRequest("No filename in metadata".to_string()))?;

        let file_only = filename
            .split(&['\\', '/'][..])
            .next_back()
            .ok_or_else(|| AppError::BadRequest("Invalid filename format".to_string()))?
            .to_string();

        (image, file_only, target_name)
    };

    // Parse parameters
    let num_stars = options.num_stars.unwrap_or(9);
    let psf_type_str = options.psf_type.as_deref().unwrap_or("moffat").to_string();
    let sort_by = options.sort_by.as_deref().unwrap_or("r2").to_string();
    let selection = options.selection.as_deref().unwrap_or("top-n").to_string();
    let grid_cols = options.grid_cols;

    let psf_type: PSFType = psf_type_str.parse().unwrap_or(PSFType::Moffat4);

    // Create comprehensive cache key for PSF multi image
    let cache_key = format!(
        "psf_multi_{}_{}_{}_{}_{}_{}_{}_{}_{}_{}",
        image_id,
        image.project_id,
        image.target_id,
        image.acquired_date.unwrap_or(0),
        file_only.replace(&['.', ' ', '-'][..], "_"),
        num_stars,
        psf_type_str,
        sort_by,
        selection,
        grid_cols.unwrap_or(0)
    );
    let cache_manager = CacheManager::new(PathBuf::from(&state.cache_dir));
    cache_manager
        .ensure_category_dir("psf_multi")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("psf_multi", &cache_key, "png");

    // Check if cached version exists
    if cache_manager.is_cached(&cache_path) {
        // Serve from cache
        let mut file = File::open(&cache_path)
            .await
            .map_err(|_| AppError::InternalError("Failed to read cache".to_string()))?;

        let mut buffer = Vec::new();
        file.read_to_end(&mut buffer)
            .await
            .map_err(|_| AppError::InternalError("Failed to read file".to_string()))?;

        return Ok((
            StatusCode::OK,
            [
                (CONTENT_TYPE, "image/png"),
                (CACHE_CONTROL, "max-age=86400"), // TODO: Use configurable cache expiry
            ],
            buffer,
        ));
    }

    // Find FITS file path first (this is fast)
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;

    // Move expensive operations to spawn_blocking
    let fits_path_str = fits_path.to_string_lossy().to_string();
    let cache_path_clone = cache_path.clone();
    tokio::task::spawn_blocking(move || {
        // Load FITS file
        let fits = FitsImage::from_file(std::path::Path::new(&fits_path_str))
            .map_err(|e| anyhow::anyhow!("Failed to load FITS: {}", e))?;

        // Create PSF multi visualization using the common function
        let rgba_image =
            create_psf_multi_image(&fits, num_stars, psf_type, &sort_by, grid_cols, &selection)
                .map_err(|e| anyhow::anyhow!("Failed to create PSF visualization: {}", e))?;

        // Save to cache
        let cache_file = std::fs::File::create(&cache_path_clone)
            .map_err(|e| anyhow::anyhow!("Failed to create cache file: {}", e))?;
        let writer = std::io::BufWriter::new(cache_file);
        let encoder =
            PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::NoFilter);

        encoder
            .write_image(
                &rgba_image,
                rgba_image.width(),
                rgba_image.height(),
                ColorType::Rgba8.into(),
            )
            .map_err(|e| anyhow::anyhow!("Failed to encode PNG: {}", e))?;

        Ok::<(), anyhow::Error>(())
    })
    .await
    .map_err(|e| AppError::InternalError(format!("PSF visualization task panicked: {}", e)))?
    .map_err(|e| AppError::InternalError(format!("Failed to generate PSF visualization: {}", e)))?;

    // Read the cached file
    let png_buffer = tokio::fs::read(&cache_path)
        .await
        .map_err(|_| AppError::InternalError("Failed to read generated PNG".to_string()))?;

    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "image/png"),
            (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
        ],
        png_buffer,
    ))
}

// Error handling
#[derive(Debug)]
pub enum AppError {
    NotFound,
    DatabaseError,
    BadRequest(String),
    InternalError(String),
    NotImplemented,
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, error_message) = match &self {
            AppError::NotFound => {
                tracing::warn!("🔍 Resource not found");
                (StatusCode::NOT_FOUND, "Resource not found")
            }
            AppError::DatabaseError => {
                tracing::error!("💾 Database error occurred");
                (StatusCode::INTERNAL_SERVER_ERROR, "Database error")
            }
            AppError::BadRequest(msg) => {
                tracing::warn!("❌ Bad request: {}", msg);
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::InternalError(msg) => {
                tracing::error!("⚠️  Internal server error: {}", msg);
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(msg.clone())),
                )
                    .into_response();
            }
            AppError::NotImplemented => {
                tracing::debug!("🚧 Not implemented endpoint accessed");
                (StatusCode::NOT_IMPLEMENTED, "Not implemented yet")
            }
        };

        (
            status,
            Json(ApiResponse::<()>::error(error_message.to_string())),
        )
            .into_response()
    }
}
