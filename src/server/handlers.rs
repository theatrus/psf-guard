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
        image_directory: state.image_dir.clone(),
        cache_directory: state.cache_dir.clone(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    Ok(Json(ApiResponse::success(info)))
}

pub async fn refresh_file_cache(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<FileCheckResponse>>, AppError> {
    let start_time = std::time::Instant::now();

    // Clear existing cache
    {
        let mut cache = state.file_check_cache.write().unwrap();
        cache.clear();
    }

    // Refresh projects
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

    Ok(Json(ApiResponse::success(response)))
}

pub async fn list_projects(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ApiResponse<Vec<ProjectResponse>>>, AppError> {
    // Check if cache is expired
    let needs_refresh = {
        let cache = state.file_check_cache.read().unwrap();
        cache.is_expired() || cache.projects_with_files.is_empty()
    };

    if needs_refresh {
        // Refresh the cache
        refresh_project_cache(&state).await?;
    }

    // Get file existence info from cache
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

    Ok(Json(ApiResponse::success(response)))
}

async fn refresh_project_cache(state: &Arc<AppState>) -> Result<(), AppError> {
    let projects_to_check = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);
        db.get_projects_with_images()
            .map_err(|_| AppError::DatabaseError)?
    };

    let mut cache_updates = HashMap::new();

    for project in projects_to_check {
        // Check a few sample images for this project
        let has_files = check_project_has_files(state, project.id).await?;
        cache_updates.insert(project.id, has_files);
    }

    // Update cache
    {
        let mut cache = state.file_check_cache.write().unwrap();
        cache.projects_with_files = cache_updates;
        cache.last_updated = std::time::Instant::now();
    }

    Ok(())
}

async fn check_project_has_files(state: &Arc<AppState>, project_id: i32) -> Result<bool, AppError> {
    let images_to_check = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);

        db.query_images(None, None, None, None)
            .map_err(|_| AppError::DatabaseError)?
            .into_iter()
            .filter(|(img, _, _)| img.project_id == project_id)
            .take(5) // Check up to 5 images
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

pub async fn list_targets(
    State(state): State<Arc<AppState>>,
    Path(project_id): Path<i32>,
) -> Result<Json<ApiResponse<Vec<TargetResponse>>>, AppError> {
    // Check if we need to refresh target cache for this project
    let needs_refresh = {
        let cache = state.file_check_cache.read().unwrap();
        cache.is_expired()
            || !cache.targets_with_files.iter().any(|(_tid, _)| {
                // Check if we have any cached data for targets in this project
                true // We'll need to track project_id per target for more efficient caching
            })
    };

    if needs_refresh {
        // Refresh target cache for this project
        refresh_target_cache(&state, project_id).await?;
    }

    // Get file existence info from cache
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

    Ok(Json(ApiResponse::success(response)))
}

async fn refresh_target_cache(state: &Arc<AppState>, project_id: i32) -> Result<(), AppError> {
    let targets_to_check = {
        let conn = state.db();
        let conn = conn.lock().map_err(|_| AppError::DatabaseError)?;
        let db = Database::new(&conn);
        db.get_targets_with_images(project_id)
            .map_err(|_| AppError::DatabaseError)?
    };

    let mut cache_updates = HashMap::new();

    for (target, _, _, _) in targets_to_check {
        // Check a few sample images for this target
        let has_files = check_target_has_files(state, target.id).await?;
        cache_updates.insert(target.id, has_files);
    }

    // Update cache
    {
        let mut cache = state.file_check_cache.write().unwrap();
        // Merge updates into existing cache
        for (tid, has_files) in cache_updates {
            cache.targets_with_files.insert(tid, has_files);
        }
        cache.last_updated = std::time::Instant::now();
    }

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
    let stats_cache_filename = format!("stats_{}.json", image_id);
    let stats_cache_path = state.get_cache_path("stats", &stats_cache_filename);

    // Ensure cache directory exists
    if let Some(parent) = stats_cache_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }

    // Check if statistics are already cached
    let fits_stats = if tokio::fs::metadata(&stats_cache_path).await.is_ok() {
        // Load from cache
        match tokio::fs::read_to_string(&stats_cache_path).await {
            Ok(cached_data) => serde_json::from_str::<serde_json::Value>(&cached_data).ok(),
            Err(_) => None,
        }
    } else {
        // Calculate statistics from FITS file
        let filename_result = metadata["FileName"].as_str().and_then(|filename| {
            filename
                .split(&['\\', '/'][..])
                .next_back()
                .map(|file_only| find_fits_file(&state, &image, &target_name, file_only))
        });

        if let Some(Ok(fits_path)) = filename_result {
            if let Ok(fits) = FitsImage::from_file(&fits_path) {
                let stats = fits.calculate_basic_statistics();

                // Extract temperature and camera model from FITS headers
                let temperature = FitsImage::extract_temperature(&fits_path);
                let camera_model = FitsImage::extract_camera_model(&fits_path);

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
        }
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
    let size = options.size.as_deref().unwrap_or("screen");
    let stretch = options.stretch.unwrap_or(true);
    let midtone = options.midtone.unwrap_or(0.2);
    let shadow = options.shadow.unwrap_or(-2.8);

    // Create cache key
    let cache_key = format!(
        "{}_{}_{}_{}_{}",
        image_id,
        size,
        if stretch { "stretched" } else { "linear" },
        (midtone * 1000.0) as i32,
        (shadow * 1000.0) as i32
    );

    let cache_manager = CacheManager::new(PathBuf::from(&state.cache_dir));
    cache_manager
        .ensure_category_dir("previews")
        .map_err(|e| AppError::InternalError(format!("Failed to create cache directory: {}", e)))?;
    let cache_path = cache_manager.get_cached_path("previews", &cache_key, "png");

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
                (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
            ],
            buffer,
        ));
    }

    // Find the FITS file
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;

    // Load FITS file (just to verify it exists and is valid)
    let _fits = FitsImage::from_file(&fits_path)
        .map_err(|e| AppError::InternalError(format!("Failed to load FITS: {}", e)))?;

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
    stretch_to_png_with_resize(
        &fits_path.to_string_lossy(),
        Some(cache_path_str.clone()),
        midtone,
        shadow,
        false, // logarithmic
        false, // invert
        max_dimensions,
    )
    .map_err(|e| AppError::InternalError(format!("Failed to generate preview: {}", e)))?;

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

// Helper function to find FITS file
fn find_fits_file(
    state: &AppState,
    image: &crate::models::AcquiredImage,
    target_name: &str,
    filename: &str,
) -> Result<std::path::PathBuf, AppError> {
    use crate::commands::filter_rejected::{find_file_recursive, get_possible_paths};

    // Extract date from acquired_date
    let acquired_date = image
        .acquired_date
        .and_then(|d| chrono::DateTime::from_timestamp(d, 0))
        .ok_or_else(|| AppError::BadRequest("Invalid date".to_string()))?;

    let date_str = acquired_date.format("%Y-%m-%d").to_string();

    // Try to find the file in different possible locations
    let possible_paths = get_possible_paths(&state.image_dir, &date_str, target_name, filename);

    for path in &possible_paths {
        if path.exists() {
            return Ok(path.clone());
        }
    }

    // Try recursive search as fallback
    match find_file_recursive(&state.image_dir, filename)
        .map_err(|e| AppError::InternalError(format!("Search failed: {}", e)))?
    {
        Some(path) => Ok(path),
        None => Err(AppError::NotFound),
    }
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

    // Create cache key for star detection results
    let cache_key = format!("stars_{}", image_id);
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

    // Find and load the FITS file
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;
    let fits = FitsImage::from_file(&fits_path)
        .map_err(|e| AppError::InternalError(format!("Failed to load FITS: {}", e)))?;

    // Run star detection
    let params = HocusFocusParams {
        psf_type: PSFType::Moffat4,
        ..Default::default()
    };

    let detection_result = detect_stars_hocus_focus(&fits.data, fits.width, fits.height, &params);

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

    let response = StarDetectionResponse {
        detected_stars: detection_result.stars.len(),
        average_hfr: detection_result.average_hfr,
        average_fwhm: detection_result.average_fwhm,
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

    // Create cache key for annotated image
    let cache_key = format!("annotated_{}_{}", image_id, size);
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
                (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
            ],
            buffer,
        ));
    }

    // Find and load the FITS file
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;
    let fits = FitsImage::from_file(&fits_path)
        .map_err(|e| AppError::InternalError(format!("Failed to load FITS: {}", e)))?;

    // Create annotated image using the common function
    let rgb_image = create_annotated_image(
        &fits,
        100,                // max_stars
        0.2,                // midtone_factor
        -2.8,               // shadow_clipping
        Rgb([255, 255, 0]), // yellow color
    )
    .map_err(|e| AppError::InternalError(format!("Failed to create annotated image: {}", e)))?;

    // Resize if needed based on size parameter
    let final_image = match size {
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
    let psf_type_str = options.psf_type.as_deref().unwrap_or("moffat");
    let sort_by = options.sort_by.as_deref().unwrap_or("r2");
    let selection = options.selection.as_deref().unwrap_or("top-n");

    let psf_type: PSFType = psf_type_str.parse().unwrap_or(PSFType::Moffat4);

    // Create cache key for PSF multi image
    let cache_key = format!(
        "psf_multi_{}_{}_{}_{}_{}_{}",
        image_id,
        num_stars,
        psf_type_str,
        sort_by,
        selection,
        options.grid_cols.unwrap_or(0)
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
                (CACHE_CONTROL, "max-age=86400"), // Cache for 1 day
            ],
            buffer,
        ));
    }

    // Find and load the FITS file
    let fits_path = find_fits_file(&state, &image, &target_name, &file_only)?;
    let fits = FitsImage::from_file(&fits_path)
        .map_err(|e| AppError::InternalError(format!("Failed to load FITS: {}", e)))?;

    // Create PSF multi visualization using the common function
    let rgba_image = create_psf_multi_image(
        &fits,
        num_stars,
        psf_type,
        sort_by,
        options.grid_cols,
        selection,
    )
    .map_err(|e| AppError::InternalError(format!("Failed to create PSF visualization: {}", e)))?;

    // Save to cache
    let cache_file = std::fs::File::create(&cache_path)
        .map_err(|e| AppError::InternalError(format!("Failed to create cache file: {}", e)))?;
    let writer = std::io::BufWriter::new(cache_file);
    let encoder = PngEncoder::new_with_quality(writer, CompressionType::Fast, FilterType::NoFilter);

    encoder
        .write_image(
            &rgba_image,
            rgba_image.width(),
            rgba_image.height(),
            ColorType::Rgba8.into(),
        )
        .map_err(|e| AppError::InternalError(format!("Failed to encode PNG: {}", e)))?;

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
        let (status, error_message) = match self {
            AppError::NotFound => (StatusCode::NOT_FOUND, "Resource not found"),
            AppError::DatabaseError => (StatusCode::INTERNAL_SERVER_ERROR, "Database error"),
            AppError::BadRequest(msg) => {
                return (StatusCode::BAD_REQUEST, Json(ApiResponse::<()>::error(msg)))
                    .into_response()
            }
            AppError::InternalError(msg) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(msg)),
                )
                    .into_response()
            }
            AppError::NotImplemented => (StatusCode::NOT_IMPLEMENTED, "Not implemented yet"),
        };

        (
            status,
            Json(ApiResponse::<()>::error(error_message.to_string())),
        )
            .into_response()
    }
}
